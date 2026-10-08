use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub server: String,
    pub agent_id: String,
    pub credential: Option<String>,
    #[serde(default)]
    pub pending_credential: Option<String>,
    pub ca_pem: Option<String>,
}
pub fn directory() -> PathBuf {
    std::env::var_os("HOPE_AGENT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local/state/hope-agent")
        })
}
pub async fn read(dir: &Path) -> Result<Config> {
    serde_json::from_slice(
        &tokio::fs::read(dir.join("config.json"))
            .await
            .context("Agent is not enrolled. Run the H.O.P.E installer first.")?,
    )
    .context("Invalid agent configuration")
}
pub async fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path.parent().context("No state directory")?;
    tokio::fs::create_dir_all(parent).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).await?;
    }
    let temporary = path.with_extension("tmp");
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary).await?;
    file.write_all(data).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(temporary, path).await?;
    Ok(())
}
pub async fn save(dir: &Path, config: &Config) -> Result<()> {
    write_private(&dir.join("config.json"), &serde_json::to_vec(config)?).await
}
pub fn client(config: &Config) -> Result<reqwest::Client> {
    let url = reqwest::Url::parse(&config.server)?;
    anyhow::ensure!(
        url.scheme() == "https"
            || (url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))),
        "Reporting requires HTTPS; HTTP is only permitted on localhost for development"
    );
    anyhow::ensure!(
        url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none(),
        "Server must be a URL origin without credentials, query, or path"
    );
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("hope-agent/", env!("CARGO_PKG_VERSION")));
    if let Some(pem) = &config.ca_pem {
        builder = builder.add_root_certificate(reqwest::Certificate::from_pem(pem.as_bytes())?);
    }
    Ok(builder.build()?)
}
