//! Reserve report order durably before queueing a sample. Gaps after a crash are harmless.
use crate::{config, delivery::Spool};
use anyhow::{Context, Result, ensure};
use hope_protocol::MAX_REPORT_SEQUENCE;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize)]
pub struct Sequence {
    agent_id: String,
    reserved: u64,
}

impl Sequence {
    pub async fn load(dir: &Path, agent_id: &str, spool: &Spool) -> Result<Self> {
        let path = dir.join("report-sequence.json");
        let stored: Option<Self> = match config::read_bounded(&path, 1024).await {
            Ok(bytes) => {
                Some(serde_json::from_slice(&bytes).context("Invalid report sequence state")?)
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                None
            }
            Err(error) => return Err(error.context("Could not read report sequence state")),
        };
        let reserved = stored
            .filter(|state| state.agent_id == agent_id)
            .map_or(0, |state| state.reserved)
            .max(
                spool
                    .iter()
                    .filter(|report| report.agent_id == agent_id)
                    .filter_map(|report| report.sequence)
                    .max()
                    .unwrap_or(0),
            );
        ensure!(
            reserved <= MAX_REPORT_SEQUENCE,
            "Invalid report sequence watermark"
        );
        Ok(Self {
            agent_id: agent_id.into(),
            reserved,
        })
    }

    pub async fn next(&mut self, dir: &Path) -> Result<u64> {
        ensure!(
            self.reserved < MAX_REPORT_SEQUENCE,
            "Report sequence exhausted"
        );
        self.advance(dir, self.reserved + 1).await?;
        Ok(self.reserved)
    }

    /// A restored local backup may be behind the server. Only newly sampled reports use this
    /// recovered watermark; queued history is never renumbered into fresh telemetry.
    pub async fn advance(&mut self, dir: &Path, watermark: u64) -> Result<()> {
        ensure!(
            watermark <= MAX_REPORT_SEQUENCE,
            "Invalid server report sequence watermark"
        );
        if watermark > self.reserved {
            self.reserved = watermark;
            config::write_private(
                &dir.join("report-sequence.json"),
                &serde_json::to_vec(self)?,
            )
            .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_counter_recovers_from_queued_reports_without_renumbering_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::default();
        let report = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "report_id": "queued", "agent_id": "agent",
            "sequence": 40, "collected_at": 100, "inventory": null,
            "metrics": hope_protocol::Metrics::default(), "collectors": []
        }))
        .unwrap();
        spool.push(report, 100.0).unwrap();
        let mut counter = Sequence::load(dir.path(), "agent", &spool).await.unwrap();
        assert_eq!(counter.next(dir.path()).await.unwrap(), 41);
        assert_eq!(spool.latest().unwrap().sequence, Some(40));
        let mut other = Sequence::load(dir.path(), "other", &spool).await.unwrap();
        assert_eq!(other.next(dir.path()).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn reservation_survives_restart_and_server_backup_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let spool = Spool::default();
        let mut counter = Sequence::load(dir.path(), "agent", &spool).await.unwrap();
        assert_eq!(counter.next(dir.path()).await.unwrap(), 1);
        // The report was never queued: its reserved position still cannot be reused.
        let mut restarted = Sequence::load(dir.path(), "agent", &spool).await.unwrap();
        assert_eq!(restarted.next(dir.path()).await.unwrap(), 2);
        restarted.advance(dir.path(), 90).await.unwrap();
        restarted.advance(dir.path(), 5).await.unwrap();
        let mut restored = Sequence::load(dir.path(), "agent", &spool).await.unwrap();
        assert_eq!(restored.next(dir.path()).await.unwrap(), 91);
        assert!(
            restored
                .advance(dir.path(), MAX_REPORT_SEQUENCE + 1)
                .await
                .is_err()
        );
        let mut other_identity = Sequence::load(dir.path(), "another-agent", &spool)
            .await
            .unwrap();
        assert_eq!(other_identity.next(dir.path()).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn corrupt_or_unreadable_state_cannot_silently_restart_ordering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report-sequence.json");
        tokio::fs::write(&path, "broken").await.unwrap();
        assert!(
            Sequence::load(dir.path(), "agent", &Spool::default())
                .await
                .is_err()
        );
        tokio::fs::remove_file(&path).await.unwrap();
        tokio::fs::create_dir(&path).await.unwrap();
        assert!(
            Sequence::load(dir.path(), "agent", &Spool::default())
                .await
                .is_err()
        );
    }
}
