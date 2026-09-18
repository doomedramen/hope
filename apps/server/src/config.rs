use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};

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
}

fn defaults() -> Config {
    Config {
        bind_addr: "0.0.0.0:8080".to_string(),
        database_url: String::new(),
        web_dist_dir: "apps/web/dist".to_string(),
    }
}

impl Config {
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

        if config.database_url.is_empty() {
            anyhow::bail!("database_url is required: set HOPE_DATABASE_URL or config/default.toml");
        }

        Ok(config)
    }
}
