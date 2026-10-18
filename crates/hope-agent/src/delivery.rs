use anyhow::Result;
use hope_protocol::{AgentReport, ErrorResponse, ReportResponse};
use reqwest::{Client, StatusCode};
use std::{
    collections::VecDeque,
    time::{Duration, SystemTime},
};

#[derive(Debug)]
pub struct Revoked;
impl std::fmt::Display for Revoked {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Agent credential was revoked or rejected; request an identity-bound replacement from the administrator")
    }
}
impl std::error::Error for Revoked {}

pub const MAX_BYTES: usize = 4 * 1024 * 1024;
/// One hour of reports at the shortest interval the server can choose (five seconds).
pub const MAX_REPORTS: usize = 720;
pub const MAX_AGE_SECONDS: f64 = 3600.0;
// Reject a count-overflowing persisted queue while deserializing, before an
// attacker-controlled small JSON file expands into thousands of report structs.
struct BoundedReports(VecDeque<AgentReport>);
impl<'de> serde::Deserialize<'de> for BoundedReports {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = BoundedReports;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a bounded report queue")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut reports = VecDeque::new();
                while let Some(report) = sequence.next_element::<AgentReport>()? {
                    if reports.len() == MAX_REPORTS {
                        return Err(serde::de::Error::custom("report queue count exceeded"));
                    }
                    reports.push_back(report);
                }
                Ok(BoundedReports(reports))
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}
#[derive(Default)]
pub struct Spool {
    pub reports: VecDeque<AgentReport>,
}
impl Spool {
    pub fn decode(bytes: &[u8], identity: &str, now: f64) -> Self {
        let reports = if bytes.len() <= MAX_BYTES {
            serde_json::from_slice::<BoundedReports>(bytes)
                .map(|bounded| bounded.0)
                .unwrap_or_default()
        } else {
            VecDeque::new()
        };
        let mut spool = Self { reports };
        spool.reports.retain(|r| r.agent_id == identity);
        let _ = spool.trim(now);
        spool
    }
    pub fn push(&mut self, report: AgentReport, now: f64) -> Result<()> {
        self.reports.push_back(report);
        self.trim(now)
    }
    pub fn trim(&mut self, now: f64) -> Result<()> {
        self.reports.retain(|r| {
            r.collected_at.is_finite()
                && r.collected_at > now - MAX_AGE_SECONDS
                && r.collected_at <= now + 300.0
        });
        while self.reports.len() > MAX_REPORTS || self.encode()?.len() > MAX_BYTES {
            self.reports.pop_front();
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&self.reports)?)
    }
    pub fn remove(&mut self, id: &str) {
        self.reports.retain(|r| r.report_id != id);
    }
}
#[derive(Debug)]
pub enum Outcome {
    Accepted(ReportResponse),
    Retry(Option<Duration>),
    Invalid(u16),
    Revoked,
}
pub fn retry_after(value: Option<&str>, now: SystemTime) -> Option<Duration> {
    let text = value?;
    let seconds = text.parse::<u64>().ok().or_else(|| {
        httpdate::parse_http_date(text)
            .ok()
            .map(|time| time.duration_since(now).unwrap_or_default().as_secs())
    })?;
    Some(Duration::from_secs(seconds.clamp(1, 900)))
}
pub fn backoff(failures: u32, jitter: u64) -> Duration {
    Duration::from_secs(
        (30u64.saturating_mul(2u64.saturating_pow(failures.min(5))) + jitter % 31).min(900),
    )
}
pub async fn send(
    client: Client,
    server: String,
    credential: String,
    report: AgentReport,
) -> (AgentReport, Outcome) {
    let outcome = match client
        .post(format!("{server}/api/v1/agent/report"))
        .bearer_auth(credential)
        .json(&report)
        .send()
        .await
    {
        Ok(response)
            if response.status() == StatusCode::UNAUTHORIZED
                || response.status() == StatusCode::FORBIDDEN =>
        {
            // A proxy's denial (or an older server's generic authentication error)
            // must not permanently stop the service. Only an explicit server
            // revocation response is terminal.
            match bounded_json::<ErrorResponse>(response).await {
                Ok(error) if error.code == "agent_revoked" => Outcome::Revoked,
                _ => Outcome::Retry(None),
            }
        }
        Ok(response)
            if response.status() == StatusCode::TOO_MANY_REQUESTS
                || response.status().is_server_error() =>
        {
            Outcome::Retry(retry_after(
                response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok()),
                SystemTime::now(),
            ))
        }
        Ok(response) if response.status().is_client_error() => {
            Outcome::Invalid(response.status().as_u16())
        }
        Ok(response) if response.status().is_success() => {
            match bounded_json::<ReportResponse>(response).await {
                Ok(accepted) if accepted.accepted => Outcome::Accepted(accepted),
                _ => Outcome::Retry(None),
            }
        }
        _ => Outcome::Retry(None),
    };
    (report, outcome)
}
pub async fn bounded_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T> {
    let mut bytes = vec![];
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            bytes.len() + chunk.len() <= 16384,
            "Server response exceeds the supported size"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}
#[cfg(test)]
mod tests {
    use super::*;
    use hope_protocol::{Metrics, SCHEMA_VERSION};
    fn report(time: f64) -> AgentReport {
        AgentReport {
            schema_version: SCHEMA_VERSION,
            report_id: uuid::Uuid::new_v4().to_string(),
            agent_id: "identity".into(),
            collected_at: time,
            inventory: None,
            metrics: Metrics::default(),
            collectors: vec![],
        }
    }
    #[test]
    fn count_age_identity_and_corrupt_buffer_are_bounded() {
        let mut spool = Spool::default();
        for time in 0..MAX_REPORTS + 80 {
            spool.push(report(time as f64), 1000.0).expect("push");
        }
        assert_eq!(spool.reports.len(), MAX_REPORTS);
        spool.trim(5000.0).expect("trim");
        assert!(spool.reports.is_empty());
        assert!(
            Spool::decode(b"invalid", "identity", 200.0)
                .reports
                .is_empty()
        );
        assert!(
            Spool::decode(
                &serde_json::to_vec(&VecDeque::from([report(100.0)])).expect("json"),
                "other",
                200.0
            )
            .reports
            .is_empty()
        );
        assert!(
            Spool::decode(&vec![0; MAX_BYTES + 1], "identity", 200.0)
                .reports
                .is_empty()
        );
    }
    #[test]
    fn runtime_byte_budget_and_clock_skew_evict_oldest_samples() {
        let mut spool = Spool::default();
        for index in 0..30 {
            let mut sample = report(100.0);
            sample.collectors.push(hope_protocol::CollectorStatus {
                name: format!("{index}"),
                available: false,
                detail: Some("x".repeat(200_000)),
            });
            spool.push(sample, 100.0).expect("push");
        }
        assert!(spool.encode().expect("encode").len() <= MAX_BYTES);
        assert!(spool.reports.len() < 30);
        assert_eq!(
            spool.reports.back().expect("latest").collectors[0].name,
            "29"
        );
        spool.push(report(1000.0), 100.0).expect("future");
        assert!(
            spool
                .reports
                .iter()
                .all(|sample| sample.collected_at <= 400.0)
        );
        let oversized_count = vec![report(100.0); MAX_REPORTS + 1];
        assert!(
            Spool::decode(
                &serde_json::to_vec(&oversized_count).expect("encode"),
                "identity",
                100.0
            )
            .reports
            .is_empty()
        );
    }
    #[test]
    fn retry_delays_are_bounded_and_understand_http_dates() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10000);
        assert_eq!(
            retry_after(Some("999999"), now),
            Some(Duration::from_secs(900))
        );
        assert_eq!(
            retry_after(
                Some(&httpdate::fmt_http_date(now + Duration::from_secs(60))),
                now
            ),
            Some(Duration::from_secs(60))
        );
        assert_eq!(retry_after(Some("invalid"), now), None);
        for failures in 0..100 {
            assert!(backoff(failures, 30) <= Duration::from_secs(900));
        }
    }
}
