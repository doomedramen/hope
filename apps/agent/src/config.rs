use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};

/// Default on-disk location for the agent's enrolled identity (private
/// key, signed cert, CA cert) on a monitored host.
pub const DEFAULT_STATE_DIR: &str = "/var/lib/hope";

/// Agent configuration. No mTLS material is loaded yet (that's the next
/// slice, per ADR-0007) — this only covers what the version/config-print
/// stub needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server_url: String,
}

fn defaults() -> Config {
    Config {
        server_url: String::new(),
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let mut figment = Figment::from(Serialized::defaults(defaults()));
        if std::path::Path::new("config/agent.toml").exists() {
            figment = figment.merge(Toml::file("config/agent.toml"));
        }
        if let Ok(path) = std::env::var("HOPE_AGENT_CONFIG_FILE") {
            figment = figment.merge(Toml::file(path));
        }
        figment = figment.merge(Env::prefixed("HOPE_AGENT_"));
        Ok(figment.extract()?)
    }
}
