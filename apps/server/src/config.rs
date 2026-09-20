use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};

pub(crate) const MIN_RETENTION_DAYS: i64 = 1;
// Ten years permits long-lived history without allowing accidental interval
// values that make a retention job remove nearly all stored data.
pub(crate) const MAX_RETENTION_DAYS: i64 = 3650;

pub(crate) const DEFAULT_RAW_MONITOR_RETENTION_DAYS: i64 = 30;
pub(crate) const DEFAULT_MONITOR_ROLLUP_AFTER_DAYS: i64 = 7;
pub(crate) const DEFAULT_MONITOR_ROLLUP_RETENTION_DAYS: i64 = 365;
pub(crate) const DEFAULT_CHANGE_EVENT_RETENTION_DAYS: i64 = 365;
pub(crate) const DEFAULT_AUDIT_EVENT_RETENTION_DAYS: i64 = 365;
pub(crate) const DEFAULT_JOB_RETENTION_DAYS: i64 = 30;

/// Server configuration, loaded from (in increasing priority order):
/// 1. `config/default.toml` if present,
/// 2. a file named by `HOPE_CONFIG_FILE` if set,
/// 3. environment variables prefixed `HOPE_` (e.g. `HOPE_DATABASE_URL`).
///
/// No field here has a default secret value: `database_url` must come from
/// a file or the environment, never a hard-coded fallback (spec §17 M0
/// acceptance gate). Session state is server-side (see `AppState`), so the
/// session cookie itself carries no secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub bind_addr: String,
    pub database_url: String,
    pub web_dist_dir: String,

    /// Agent gateway (mTLS WebSocket, ADR-0007/0008) and enrollment (TLS,
    /// server-auth only) listener addresses.
    pub gateway_bind_addr: String,
    pub enroll_bind_addr: String,

    /// PKI material paths. Never defaulted to embedded/checked-in key
    /// material — `server ca init` writes real files here; nothing is
    /// generated until that command runs.
    pub ca_cert_path: String,
    pub ca_key_path: String,
    pub server_cert_path: String,
    pub server_key_path: String,

    /// Whether the session cookie gets the `Secure` attribute. Defaults to
    /// true; set to false only for plain-HTTP local development, where a
    /// browser would otherwise silently drop the cookie.
    pub cookie_secure: bool,

    /// Whether to trust `X-Forwarded-For` for rate-limiting's client-IP
    /// determination. Only enable this when the server sits behind a
    /// reverse proxy that overwrites/strips that header itself — otherwise
    /// a client can spoof their rate-limit bucket. Defaults to false (use
    /// the TCP peer address).
    pub trust_proxy_headers: bool,

    /// `change_events` retention window (design docs/design/m1-inventory.md
    /// §5, Decision 5). The `change_events.retention` worker job deletes
    /// rows older than this many days, in bounded batches.
    pub change_event_retention_days: i64,

    /// Raw monitor results older than this many days are deleted after they
    /// have been included in an hourly rollup. Defaults to 30 days (spec
    /// §15.3).
    pub monitor_result_retention_days: i64,

    /// Raw monitor results older than this many days are compacted into
    /// hourly rollups.
    pub monitor_result_rollup_after_days: i64,

    /// Hourly monitor rollups older than this many days are deleted.
    /// Defaults to one year (spec §15.3).
    pub monitor_result_rollup_retention_days: i64,

    /// Audit events older than this many days are deleted in bounded batches.
    /// Defaults to one year (spec §15.3).
    pub audit_event_retention_days: i64,

    /// Terminal job records older than this many days are deleted in bounded
    /// batches. Pending and running jobs are never eligible.
    pub job_retention_days: i64,
}

fn defaults() -> Config {
    Config {
        bind_addr: "0.0.0.0:8080".to_string(),
        database_url: String::new(),
        web_dist_dir: "apps/web/dist".to_string(),
        gateway_bind_addr: "0.0.0.0:8443".to_string(),
        enroll_bind_addr: "0.0.0.0:8444".to_string(),
        ca_cert_path: "data/pki/ca-cert.pem".to_string(),
        ca_key_path: "data/pki/ca-key.pem".to_string(),
        server_cert_path: "data/pki/server-cert.pem".to_string(),
        server_key_path: "data/pki/server-key.pem".to_string(),
        cookie_secure: true,
        trust_proxy_headers: false,
        change_event_retention_days: DEFAULT_CHANGE_EVENT_RETENTION_DAYS,
        monitor_result_retention_days: DEFAULT_RAW_MONITOR_RETENTION_DAYS,
        monitor_result_rollup_after_days: DEFAULT_MONITOR_ROLLUP_AFTER_DAYS,
        monitor_result_rollup_retention_days: DEFAULT_MONITOR_ROLLUP_RETENTION_DAYS,
        audit_event_retention_days: DEFAULT_AUDIT_EVENT_RETENTION_DAYS,
        job_retention_days: DEFAULT_JOB_RETENTION_DAYS,
    }
}

impl Config {
    fn validate(&self) -> anyhow::Result<()> {
        for (name, value) in [
            (
                "change_event_retention_days",
                self.change_event_retention_days,
            ),
            (
                "monitor_result_retention_days",
                self.monitor_result_retention_days,
            ),
            (
                "monitor_result_rollup_after_days",
                self.monitor_result_rollup_after_days,
            ),
            (
                "monitor_result_rollup_retention_days",
                self.monitor_result_rollup_retention_days,
            ),
            (
                "audit_event_retention_days",
                self.audit_event_retention_days,
            ),
            ("job_retention_days", self.job_retention_days),
        ] {
            if !(MIN_RETENTION_DAYS..=MAX_RETENTION_DAYS).contains(&value) {
                anyhow::bail!(
                    "{name} must be between {MIN_RETENTION_DAYS} and {MAX_RETENTION_DAYS} days"
                );
            }
        }

        if self.monitor_result_retention_days <= self.monitor_result_rollup_after_days {
            anyhow::bail!(
                "monitor_result_retention_days must exceed monitor_result_rollup_after_days"
            );
        }

        Ok(())
    }

    pub fn load() -> anyhow::Result<Self> {
        let mut figment = Figment::from(Serialized::defaults(defaults()));

        if std::path::Path::new("config/default.toml").exists() {
            figment = figment.merge(Toml::file("config/default.toml"));
        }
        if let Ok(path) = std::env::var("HOPE_CONFIG_FILE") {
            figment = figment.merge(Toml::file(path));
        }
        figment = figment.merge(Env::prefixed("HOPE_"));

        let config: Config = figment.extract()?;

        config.validate()?;

        if config.database_url.is_empty() {
            anyhow::bail!("database_url is required: set HOPE_DATABASE_URL or config/default.toml");
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_defaults_match_spec() {
        let config = defaults();

        assert_eq!(config.monitor_result_retention_days, 30);
        assert_eq!(config.monitor_result_rollup_retention_days, 365);
        assert_eq!(config.change_event_retention_days, 365);
        assert_eq!(config.audit_event_retention_days, 365);
        assert_eq!(config.job_retention_days, 30);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn retention_validation_rejects_non_positive_and_excessive_values() {
        let mut zero = defaults();
        zero.audit_event_retention_days = 0;
        assert!(zero.validate().is_err());

        let mut negative = defaults();
        negative.job_retention_days = -1;
        assert!(negative.validate().is_err());

        let mut excessive = defaults();
        excessive.change_event_retention_days = MAX_RETENTION_DAYS + 1;
        assert!(excessive.validate().is_err());
    }

    #[test]
    fn retention_validation_rejects_raw_window_not_covering_rollup_delay() {
        let mut config = defaults();
        config.monitor_result_retention_days = config.monitor_result_rollup_after_days;

        assert!(config.validate().is_err());
    }
}
