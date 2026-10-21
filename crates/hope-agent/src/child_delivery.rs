use anyhow::Result;
use hope_protocol::{ChildReportBatch, ChildReportResponse, ErrorResponse};
use reqwest::{Client, StatusCode};
use std::{collections::VecDeque, time::Duration};

pub const MAX_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BATCHES: usize = 120;
pub const MAX_AGE_SECONDS: f64 = 3600.0;

struct BoundedBatches(VecDeque<ChildReportBatch>);
impl<'de> serde::Deserialize<'de> for BoundedBatches {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = BoundedBatches;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a bounded child-report queue")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut batches = VecDeque::new();
                while let Some(batch) = sequence.next_element::<ChildReportBatch>()? {
                    if batches.len() == MAX_BATCHES {
                        return Err(serde::de::Error::custom("child queue count exceeded"));
                    }
                    batches.push_back(batch);
                }
                Ok(BoundedBatches(batches))
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}

#[derive(serde::Serialize)]
#[serde(transparent)]
struct QueuedBatch {
    batch: ChildReportBatch,
    #[serde(skip)]
    serialized_bytes: usize,
}
impl QueuedBatch {
    fn new(batch: ChildReportBatch) -> Result<Self> {
        let serialized_bytes = serde_json::to_vec(&batch)?.len();
        Ok(Self {
            batch,
            serialized_bytes,
        })
    }
}

#[derive(Default)]
pub struct Spool {
    batches: VecDeque<QueuedBatch>,
    content_bytes: usize,
}
impl Spool {
    pub fn decode(bytes: &[u8], identity: &str, now: f64) -> Self {
        let batches = if bytes.len() <= MAX_BYTES {
            serde_json::from_slice::<BoundedBatches>(bytes)
                .map(|bounded| bounded.0)
                .unwrap_or_default()
        } else {
            VecDeque::new()
        };
        let mut spool = Self::default();
        for batch in batches {
            if batch.agent_id == identity
                && let Ok(queued) = QueuedBatch::new(batch)
            {
                spool.content_bytes += queued.serialized_bytes;
                spool.batches.push_back(queued);
            }
        }
        spool.trim(now);
        spool
    }
    pub fn push(&mut self, batch: ChildReportBatch, now: f64) -> Result<()> {
        let queued = QueuedBatch::new(batch)?;
        self.content_bytes += queued.serialized_bytes;
        self.batches.push_back(queued);
        self.trim(now);
        Ok(())
    }
    pub fn trim(&mut self, now: f64) {
        self.retain(|batch| {
            batch.observed_at.is_finite()
                && batch.observed_at > now - MAX_AGE_SECONDS
                && batch.observed_at <= now + 300.0
        });
        while self.batches.len() > MAX_BATCHES || self.encoded_len() > MAX_BYTES {
            if let Some(batch) = self.batches.pop_front() {
                self.content_bytes -= batch.serialized_bytes;
            }
        }
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&self.batches)?)
    }
    pub fn pop(&mut self, id: &str) {
        if self
            .batches
            .front()
            .is_some_and(|batch| batch.batch.batch_id == id)
            && let Some(batch) = self.batches.pop_front()
        {
            self.content_bytes -= batch.serialized_bytes;
        }
    }
    pub fn front(&self) -> Option<&ChildReportBatch> {
        self.batches.front().map(|queued| &queued.batch)
    }
    pub fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }
    fn retain(&mut self, mut keep: impl FnMut(&ChildReportBatch) -> bool) {
        let content_bytes = &mut self.content_bytes;
        self.batches.retain(|queued| {
            if keep(&queued.batch) {
                true
            } else {
                *content_bytes -= queued.serialized_bytes;
                false
            }
        });
    }
    fn encoded_len(&self) -> usize {
        2 + self.content_bytes + self.batches.len().saturating_sub(1)
    }
}

#[derive(Debug)]
pub enum Outcome {
    Accepted,
    Retry(Option<Duration>),
    Invalid(u16),
    Revoked,
}

pub async fn send(
    client: Client,
    server: String,
    credential: String,
    batch: ChildReportBatch,
) -> (ChildReportBatch, Outcome) {
    let outcome = match client
        .post(format!("{server}/api/v1/agent/children"))
        .bearer_auth(credential)
        .json(&batch)
        .send()
        .await
    {
        Ok(response)
            if response.status() == StatusCode::UNAUTHORIZED
                || response.status() == StatusCode::FORBIDDEN =>
        {
            match crate::delivery::bounded_json::<ErrorResponse>(response).await {
                Ok(error) if error.code == "agent_revoked" => Outcome::Revoked,
                _ => Outcome::Retry(None),
            }
        }
        Ok(response)
            if response.status() == StatusCode::TOO_MANY_REQUESTS
                || response.status().is_server_error() =>
        {
            Outcome::Retry(crate::delivery::retry_after(
                response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok()),
                std::time::SystemTime::now(),
            ))
        }
        Ok(response) if response.status().is_client_error() => {
            Outcome::Invalid(response.status().as_u16())
        }
        Ok(response) if response.status().is_success() => {
            match crate::delivery::bounded_json::<ChildReportResponse>(response).await {
                Ok(accepted) if accepted.accepted => Outcome::Accepted,
                _ => Outcome::Retry(None),
            }
        }
        _ => Outcome::Retry(None),
    };
    (batch, outcome)
}

pub fn backoff(failures: u32, jitter: u64) -> Duration {
    Duration::from_secs(
        (30u64.saturating_mul(2u64.saturating_pow(failures.min(5))) + jitter % 31).min(900),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use hope_protocol::{ChildSourceKind, SCHEMA_VERSION};

    fn batch(sequence: u64, at: f64) -> ChildReportBatch {
        ChildReportBatch {
            schema_version: SCHEMA_VERSION,
            batch_id: uuid::Uuid::new_v4().to_string(),
            agent_id: "agent".into(),
            source: ChildSourceKind::Docker,
            source_scope: "engine".into(),
            source_token: None,
            legacy_source_scope: None,
            legacy_identities: vec![],
            proxmox_guest_presence: None,
            sequence,
            inventory_generation: sequence,
            inventory_id: None,
            chunk_index: 0,
            chunk_count: 1,
            complete: sequence % 2 == 1,
            observed_at: at,
            devices: vec![],
        }
    }

    #[test]
    fn child_spool_preserves_fifo_retries_and_is_bounded_by_identity_and_age() {
        let mut spool = Spool::default();
        let first = batch(1, 100.0);
        let first_id = first.batch_id.clone();
        spool.push(first, 200.0).expect("first batch");
        spool.push(batch(2, 110.0), 200.0).expect("second batch");
        let encoded = spool.encode().expect("encode");
        let mut restored = Spool::decode(&encoded, "agent", 200.0);
        assert_eq!(restored.front().unwrap().sequence, 1);
        restored.pop(&first_id);
        assert_eq!(restored.front().unwrap().sequence, 2);
        assert!(Spool::decode(&encoded, "other-agent", 200.0).is_empty());
        assert!(Spool::decode(&encoded, "agent", 10_000.0).is_empty());
        assert!(Spool::decode(&vec![0; MAX_BYTES + 1], "agent", 200.0).is_empty());
    }
}
