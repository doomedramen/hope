use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
        &read_bounded(&dir.join("config.json"), 128 * 1024)
            .await
            .context("Agent is not enrolled. Run the H.O.P.E installer first.")?,
    )
    .context("Invalid agent configuration")
}
pub fn validate_identity(configuration: &Config) -> Result<()> {
    uuid::Uuid::parse_str(&configuration.agent_id).context("Invalid agent identity")?;
    for credential in [&configuration.credential, &configuration.pending_credential]
        .into_iter()
        .flatten()
    {
        anyhow::ensure!(
            credential.len() == 64 && credential.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Invalid credential state"
        );
    }
    let _ = client(configuration)?;
    Ok(())
}
pub async fn restore_settings(directory: &Path, previous: &Path) -> Result<()> {
    let _lock = lock(directory).await?;
    let current = read(directory).await?;
    let mut old: Config = serde_json::from_slice(&read_bounded(previous, 128 * 1024).await?)?;
    validate_identity(&current)?;
    validate_identity(&old)?;
    anyhow::ensure!(
        current.agent_id == old.agent_id && current.server == old.server,
        "Rollback must preserve the same agent and server"
    );
    // Enrollment is an independent server-side transaction. Never discard a
    // issued or pending credential when rolling back local package settings.
    old.credential = current.credential;
    old.pending_credential = current.pending_credential;
    save(directory, &old).await
}

pub async fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path.parent().context("No state directory")?;
    tokio::fs::create_dir_all(parent).await?;
    let metadata = tokio::fs::symlink_metadata(parent).await?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "State directory must not be a symbolic link"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).await?;
    }
    let temporary = parent.join(format!(".hope-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary).await?;
    let result = async {
        file.write_all(data).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temporary, path).await?;
        let parent = parent.to_owned();
        tokio::task::spawn_blocking(move || std::fs::File::open(parent)?.sync_all()).await??;
        anyhow::Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}
pub async fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let file = tokio::fs::File::open(path).await?;
    let mut data = vec![];
    file.take(maximum as u64 + 1).read_to_end(&mut data).await?;
    anyhow::ensure!(
        data.len() <= maximum,
        "State file exceeds the supported size"
    );
    Ok(data)
}
pub async fn lock(dir: &Path) -> Result<std::fs::File> {
    tokio::fs::create_dir_all(dir).await?;
    let metadata = tokio::fs::symlink_metadata(dir).await?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "State directory must not be a symbolic link"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await?;
    }
    let path = dir.join("agent.lock");
    if tokio::fs::symlink_metadata(&path)
        .await
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        anyhow::bail!("Agent lock must not be a symbolic link");
    }
    tokio::task::spawn_blocking(move || {
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        file.try_lock()
            .context("Another agent process owns this state directory")?;
        Ok(file)
    })
    .await?
}
pub async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut termination) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _=termination.recv()=>{}, _=tokio::signal::ctrl_c()=>{} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
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
        let certificates = reqwest::Certificate::from_pem_bundle(pem.as_bytes())?;
        anyhow::ensure!(
            !certificates.is_empty(),
            "Custom CA file contains no certificates"
        );
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn state_has_one_owner_and_atomic_private_writes() {
        let directory = tempfile::tempdir().expect("temporary");
        let first = lock(directory.path()).await.expect("first owner");
        assert!(lock(directory.path()).await.is_err());
        drop(first);
        // A concurrent test may fork between our open and its exec, briefly sharing the flock.
        let mut next = lock(directory.path()).await;
        for _ in 0..50 {
            if next.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            next = lock(directory.path()).await;
        }
        let _next = next.expect("next owner");
        let path = directory.path().join("sample.json");
        write_private(&path, b"first").await.expect("first write");
        write_private(&path, b"second").await.expect("replacement");
        assert_eq!(read_bounded(&path, 6).await.expect("read"), b"second");
        assert!(read_bounded(&path, 3).await.is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                tokio::fs::metadata(&path)
                    .await
                    .expect("file")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let mut entries = tokio::fs::read_dir(directory.path())
            .await
            .expect("entries");
        while let Some(entry) = entries.next_entry().await.expect("entry") {
            assert!(!entry.file_name().to_string_lossy().ends_with(".tmp"));
        }
    }
    #[tokio::test]
    async fn settings_rollback_preserves_issued_and_pending_credentials() {
        let directory = tempfile::tempdir().expect("state");
        let identity = uuid::Uuid::new_v4().to_string();
        let old = Config {
            server: "http://localhost:8080".into(),
            agent_id: identity.clone(),
            credential: Some("a".repeat(64)),
            pending_credential: None,
            ca_pem: None,
        };
        let previous = directory.path().join("previous.json");
        write_private(&previous, &serde_json::to_vec(&old).expect("json"))
            .await
            .expect("backup");
        let current = Config {
            credential: Some("b".repeat(64)),
            pending_credential: Some("c".repeat(64)),
            ..old
        };
        save(directory.path(), &current).await.expect("current");
        restore_settings(directory.path(), &previous)
            .await
            .expect("rollback");
        let restored = read(directory.path()).await.expect("restored");
        assert!(
            restored.agent_id == identity
                && restored.credential == current.credential
                && restored.pending_credential == current.pending_credential
        );
        let foreign = Config {
            agent_id: uuid::Uuid::new_v4().to_string(),
            ..restored
        };
        write_private(&previous, &serde_json::to_vec(&foreign).expect("json"))
            .await
            .expect("foreign");
        assert!(restore_settings(directory.path(), &previous).await.is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_state_is_rejected_before_changing_target_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir().expect("temporary");
        let target = directory.path().join("target");
        tokio::fs::create_dir(&target).await.expect("target");
        tokio::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
            .await
            .expect("permissions");
        let link = directory.path().join("link");
        symlink(&target, &link).expect("link");
        assert!(
            write_private(&link.join("config.json"), b"private")
                .await
                .is_err()
        );
        assert_eq!(
            tokio::fs::metadata(target)
                .await
                .expect("target")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }
}
