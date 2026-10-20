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
        formatter.write_str("Agent is disabled after credential revocation; request an identity-bound replacement from the administrator to re-enroll it")
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
// Keep the on-disk array of reports unchanged. Only the serialized length is
// cached: byte-budget eviction must not serialize the entire backlog for each
// removed sample, or keep a second encoded copy of every report in memory.
#[derive(serde::Serialize)]
#[serde(transparent)]
struct QueuedReport {
    report: AgentReport,
    #[serde(skip)]
    serialized_bytes: usize,
}
impl QueuedReport {
    fn new(report: AgentReport) -> Result<Self> {
        let serialized_bytes = serde_json::to_vec(&report)?.len();
        Ok(Self {
            report,
            serialized_bytes,
        })
    }
}
#[derive(Default)]
pub struct Spool {
    reports: VecDeque<QueuedReport>,
    content_bytes: usize,
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
        let mut spool = Self::default();
        for report in reports {
            if report.agent_id == identity
                && let Ok(queued) = QueuedReport::new(report)
            {
                spool.content_bytes += queued.serialized_bytes;
                spool.reports.push_back(queued);
            }
        }
        spool.trim(now);
        spool
    }
    pub fn push(&mut self, report: AgentReport, now: f64) -> Result<()> {
        let queued = QueuedReport::new(report)?;
        self.content_bytes += queued.serialized_bytes;
        self.reports.push_back(queued);
        self.trim(now);
        Ok(())
    }
    pub fn trim(&mut self, now: f64) {
        self.retain(|r| {
            r.collected_at.is_finite()
                && r.collected_at > now - MAX_AGE_SECONDS
                && r.collected_at <= now + 300.0
        });
        while self.len() > MAX_REPORTS || self.encoded_len() > MAX_BYTES {
            if let Some(queued) = self.reports.pop_front() {
                self.content_bytes -= queued.serialized_bytes;
            }
        }
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&self.reports)?)
    }
    pub fn remove(&mut self, id: &str) {
        self.retain(|r| r.report_id != id);
    }
    fn retain(&mut self, mut keep: impl FnMut(&AgentReport) -> bool) {
        let content_bytes = &mut self.content_bytes;
        self.reports.retain(|queued| {
            if keep(&queued.report) {
                true
            } else {
                *content_bytes -= queued.serialized_bytes;
                false
            }
        });
    }
    fn encoded_len(&self) -> usize {
        // Two brackets and one comma between each pair of reports.
        2 + self.content_bytes + self.len().saturating_sub(1)
    }
    pub fn len(&self) -> usize {
        self.reports.len()
    }
    pub fn is_empty(&self) -> bool {
        self.reports.is_empty()
    }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &AgentReport> + ExactSizeIterator {
        self.reports.iter().map(|queued| &queued.report)
    }
    pub fn latest(&self) -> Option<&AgentReport> {
        self.iter().next_back()
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
            sequence: None,
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
        assert_eq!(spool.len(), MAX_REPORTS);
        spool.trim(5000.0);
        assert!(spool.is_empty());
        assert!(Spool::decode(b"invalid", "identity", 200.0).is_empty());
        assert!(
            Spool::decode(
                &serde_json::to_vec(&VecDeque::from([report(100.0)])).expect("json"),
                "other",
                200.0
            )
            .is_empty()
        );
        assert!(Spool::decode(&vec![0; MAX_BYTES + 1], "identity", 200.0).is_empty());
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
        assert_eq!(spool.encoded_len(), spool.encode().expect("encode").len());
        assert!(spool.len() < 30);
        assert_eq!(spool.latest().expect("latest").collectors[0].name, "29");
        spool.push(report(1000.0), 100.0).expect("future");
        assert!(spool.iter().all(|sample| sample.collected_at <= 400.0));
        let oversized_count = vec![report(100.0); MAX_REPORTS + 1];
        assert!(
            Spool::decode(
                &serde_json::to_vec(&oversized_count).expect("encode"),
                "identity",
                100.0
            )
            .is_empty()
        );
    }
    #[test]
    fn cached_size_tracks_escaped_reports_removal_expiry_and_legacy_round_trip() {
        fn assert_wire_format(spool: &Spool) {
            let bytes = spool.encode().expect("encode");
            assert_eq!(spool.encoded_len(), bytes.len());
            assert_eq!(
                bytes,
                serde_json::to_vec(&spool.iter().collect::<Vec<_>>()).expect("legacy array")
            );
        }
        let mut first = report(100.0);
        first.report_id = "duplicate".into();
        first.collectors.push(hope_protocol::CollectorStatus {
            name: "quotes\" and \\ slashes".into(),
            available: false,
            detail: Some("control\0\n\t and Unicode 🦀".into()),
        });
        let mut wrong_identity = report(100.0);
        wrong_identity.agent_id = "other".into();
        // Decoding must continue to read arrays written by earlier agents,
        // remove another identity and expire old entries before returning.
        let legacy = vec![
            first.clone(),
            wrong_identity,
            report(-4000.0),
            first,
            report(200.0),
        ];
        let mut spool = Spool::decode(
            &serde_json::to_vec(&legacy).expect("legacy queue"),
            "identity",
            200.0,
        );
        assert_eq!(spool.len(), 3);
        assert_wire_format(&spool);
        spool.remove("duplicate");
        assert_eq!(spool.len(), 1);
        assert_wire_format(&spool);
        spool.remove("absent");
        spool.push(report(250.0), 250.0).expect("push");
        assert_wire_format(&spool);
        let mut restored = Spool::decode(&spool.encode().expect("encode"), "identity", 250.0);
        assert_eq!(restored.len(), 2);
        assert_wire_format(&restored);
        restored.trim(3800.0);
        assert_eq!(restored.len(), 1);
        assert_wire_format(&restored);
        restored.trim(3850.0);
        assert!(restored.is_empty());
        assert_wire_format(&restored);
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
