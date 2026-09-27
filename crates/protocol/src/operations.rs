use crate::ValidationError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_LOG_BATCH_BYTES: usize = 64 * 1024;
pub const MAX_LOG_RECORDS: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogEntry {
    /// Stable source identity, including occurrence ordinal for identical lines.
    pub event_id: String,
    pub observed_at_unix_ms: i64,
    pub source: String,
    pub severity: String,
    pub message: String,
    pub attributes: serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogBatch {
    pub batch_id: Uuid,
    pub agent_id: Uuid,
    pub entries: Vec<LogEntry>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogBatchAck {
    pub batch_id: Uuid,
    pub accepted: bool,
    pub retryable: bool,
    pub reason: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct CollectionConfig {
    pub revision: i64,
    pub journal_units: Vec<String>,
    pub docker_containers: Vec<String>,
    /// Literal strings to redact before writing a log to disk.
    pub redact: Vec<String>,
    /// Overrides keyed by the exact journal:unit or docker:container source name.
    pub source_policies: std::collections::BTreeMap<String, LogSourcePolicy>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LogSourcePolicy {
    pub minimum_severity: String,
    pub max_events_per_minute: u32,
    pub max_bytes_per_minute: u32,
}
impl Default for LogSourcePolicy {
    fn default() -> Self {
        Self {
            minimum_severity: "debug".into(),
            max_events_per_minute: 1000,
            max_bytes_per_minute: 1024 * 1024,
        }
    }
}
impl LogSourcePolicy {
    pub fn includes(&self, severity: &str) -> bool {
        let levels = ["debug", "info", "notice", "warning", "error", "critical"];
        // Unclassified Docker and journal messages remain unknown, never guessed or silently filtered.
        match (
            levels.iter().position(|s| *s == severity),
            levels.iter().position(|s| *s == self.minimum_severity),
        ) {
            (Some(actual), Some(minimum)) => actual >= minimum,
            _ => true,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollectionConfigAck {
    pub revision: i64,
    pub accepted: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateCommand {
    pub operation_id: Uuid,
    pub version: String,
    pub deadline_seconds: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateReport {
    pub operation_id: Uuid,
    pub state: String,
    pub detail: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProtocolError {
    pub code: String,
    pub reason: String,
    #[serde(default)]
    pub retryable: bool,
}
impl LogBatch {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.agent_id.is_nil()
            || self.batch_id.is_nil()
            || self.entries.is_empty()
            || self.entries.len() > MAX_LOG_RECORDS
        {
            return Err(ValidationError::new("invalid log batch identity or length"));
        }
        for entry in &self.entries {
            if entry.event_id.is_empty()
                || entry.event_id.len() > 256
                || entry.source.len() > 256
                || entry.source.is_empty()
                || entry.message.len() > 8192
                || !entry.attributes.is_object()
                || ![
                    "debug", "info", "notice", "warning", "error", "critical", "unknown",
                ]
                .contains(&entry.severity.as_str())
            {
                return Err(ValidationError::new("invalid log entry"));
            }
        }
        if serde_json::to_vec(self)
            .map_err(|_| ValidationError::new("invalid logs"))?
            .len()
            > MAX_LOG_BATCH_BYTES
        {
            return Err(ValidationError::new("log batch exceeds byte limit"));
        }
        Ok(())
    }
}
impl CollectionConfig {
    pub fn validate(&self) -> Result<(), ValidationError> {
        for (source, policy) in &self.source_policies {
            let selected = self
                .journal_units
                .iter()
                .any(|name| source == &format!("journal:{name}"))
                || self
                    .docker_containers
                    .iter()
                    .any(|name| source == &format!("docker:{name}"));
            if !selected
                || !(1..=6000).contains(&policy.max_events_per_minute)
                || !(1024..=4 * 1024 * 1024).contains(&policy.max_bytes_per_minute)
                || !["debug", "info", "notice", "warning", "error", "critical"]
                    .contains(&policy.minimum_severity.as_str())
            {
                return Err(ValidationError::new("invalid source policy"));
            }
        }
        if self.revision < 0
            || self.journal_units.len() + self.docker_containers.len() > 16
            || self.redact.len() > 32
        {
            return Err(ValidationError::new("too many sources or redactions"));
        }
        for source in self.journal_units.iter().chain(&self.docker_containers) {
            if source.is_empty()
                || source.len() > 128
                || source.starts_with('-')
                || !source
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.@:-".contains(&c))
            {
                return Err(ValidationError::new("invalid unit or container name"));
            }
        }
        if self
            .redact
            .iter()
            .any(|text| text.is_empty() || text.len() > 256)
        {
            return Err(ValidationError::new("invalid redaction string"));
        }
        Ok(())
    }
}
impl UpdateCommand {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.operation_id.is_nil()
            || self.version.is_empty()
            || self.version.len() > 128
            || !self
                .version
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".-+".contains(&c))
            || !(30..=1800).contains(&self.deadline_seconds)
        {
            return Err(ValidationError::new("invalid update command"));
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_policy_is_bounded_and_preserves_unknown_severity() {
        let policy = LogSourcePolicy {
            minimum_severity: "warning".into(),
            ..Default::default()
        };
        assert!(!policy.includes("info"));
        assert!(policy.includes("warning"));
        assert!(policy.includes("critical"));
        assert!(policy.includes("unknown"));
        let mut config = CollectionConfig::default();
        config
            .source_policies
            .insert("journal:app.service".into(), policy);
        assert!(config.validate().is_err());
        config.journal_units.push("app.service".into());
        assert!(config.validate().is_ok());
        config
            .source_policies
            .get_mut("journal:app.service")
            .unwrap()
            .max_events_per_minute = 0;
        assert!(config.validate().is_err());
        let old: CollectionConfig = serde_json::from_str(
            r#"{"revision":1,"journal_units":[],"docker_containers":[],"redact":[]}"#,
        )
        .unwrap();
        assert!(old.source_policies.is_empty());
    }
    #[test]
    fn rejects_source_options_and_unbounded_configuration() {
        let mut config = CollectionConfig {
            journal_units: vec!["--directory=/etc".into()],
            ..Default::default()
        };
        assert!(config.validate().is_err());
        config.journal_units = vec!["hope-agent.service".into()];
        assert!(config.validate().is_ok());
        config.redact.push(String::new());
        assert!(config.validate().is_err());
    }
}
