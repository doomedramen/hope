//! Signed outbound updates. A separate root-owned systemd service performs replacement.
use crate::outbox::atomic_write;
use protocol::{UpdateCommand, UpdateReport};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path, time::Duration};

pub const ROOT_STATE: &str = "/var/lib/hope-updater";
const AGENT_PATH: &str = "/usr/local/libexec/hope-agent";
const HELPER_PATH: &str = "/usr/local/libexec/hope-agent-updater";
pub fn version() -> &'static str {
    option_env!("HOPE_AGENT_RELEASE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}
pub fn available(state_dir: &str) -> bool {
    cfg!(target_os = "linux")
        && state_dir == crate::config::DEFAULT_STATE_DIR
        && Path::new("/etc/systemd/system/hope-agent-update.timer").exists()
        && Path::new(HELPER_PATH).exists()
        && crate::release_verify::trusted_public_keys().is_some()
}
fn read_bounded(path: &Path, maximum: usize) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "update input must be a regular file"
    );
    file.take((maximum + 1) as u64).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= maximum, "update file exceeds size limit");
    Ok(bytes)
}
#[derive(Clone, Serialize, Deserialize)]
struct State {
    command: UpdateCommand,
    phase: String,
    detail: String,
}
fn save_state(root: &Path, state: &State) -> anyhow::Result<()> {
    crate::outbox::atomic_write_mode(
        &root.join("status.json"),
        &serde_json::to_vec(state)?,
        0o644,
    )?;
    Ok(())
}
async fn download(
    client: &reqwest::Client,
    url: reqwest::Url,
    max: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut response = client
        .get(url)
        .timeout(Duration::from_secs(120))
        .send()
        .await?
        .error_for_status()?;
    anyhow::ensure!(
        response.content_length().is_none_or(|n| n <= max as u64),
        "update download too large"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= max,
            "update download too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
pub async fn stage(
    client: reqwest::Client,
    gateway: String,
    state_dir: String,
    command: UpdateCommand,
) -> anyhow::Result<()> {
    // A reconnect must not start a second writer for the staged bundle.
    static STAGING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let Ok(_guard) = STAGING.try_lock() else {
        return Ok(());
    };
    command.validate()?;
    anyhow::ensure!(available(&state_dir), "updater service is not installed");
    let root = Path::new(&state_dir).join("update");
    if let Ok(bytes) = read_bounded(&root.join("request.json"), 4096)
        && let Ok(previous) = serde_json::from_slice::<UpdateCommand>(&bytes)
        && previous.operation_id == command.operation_id
    {
        return Ok(());
    }
    if let Ok(bytes) = read_bounded(&root.join("status.json"), 8192)
        && let Ok(previous) = serde_json::from_slice::<State>(&bytes)
        && previous.command.operation_id == command.operation_id
        && previous.phase == "failed"
    {
        return Ok(());
    }
    let result = async {
        let (platform, arch) = crate::release_verify::current_platform_arch();
        let mut base = reqwest::Url::parse(&gateway)?;
        base.set_scheme("https")
            .map_err(|_| anyhow::anyhow!("invalid update origin"))?;
        base.set_query(None);
        let mut files = Vec::new();
        for (name, max) in [
            ("manifest.json", 512 * 1024),
            ("manifest.json.sig", 4096),
            ("agent", 64 * 1024 * 1024),
        ] {
            base.set_path(&format!(
                "/agent/v1/releases/{}/{platform}/{arch}/{name}",
                command.version
            ));
            files.push(download(&client, base.clone(), max).await?);
        }
        let artifact = crate::release_verify::verify_release_manifest(
            &files[0],
            std::str::from_utf8(&files[1])?.trim(),
            &files[2],
        )?;
        anyhow::ensure!(
            artifact.record.version == command.version,
            "signed release version mismatch"
        );
        for (name, bytes) in ["manifest.json", "manifest.json.sig", "agent"]
            .into_iter()
            .zip(files)
        {
            atomic_write(&root.join(name), &bytes)?;
        }
        // Publishing the request last makes partially downloaded bundles ineligible.
        atomic_write(&root.join("request.json"), &serde_json::to_vec(&command)?)?;
        save_state(
            &root,
            &State {
                command: command.clone(),
                phase: "installing".into(),
                detail: "Verified bundle staged; waiting for updater service".into(),
            },
        )?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(error) = &result {
        save_state(
            &root,
            &State {
                command,
                phase: "failed".into(),
                detail: error.to_string().chars().take(400).collect(),
            },
        )?;
    }
    result
}
pub fn report(state_dir: &str) -> Option<UpdateReport> {
    let local = Path::new(state_dir).join("update");
    let request: Option<UpdateCommand> = read_bounded(&local.join("request.json"), 4096)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok());
    let root: Option<State> = read_bounded(&Path::new(ROOT_STATE).join("status.json"), 8192)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok());
    let local: Option<State> = read_bounded(&local.join("status.json"), 8192)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok());
    let state = match (root, local) {
        (Some(root), Some(local)) if root.command.operation_id != local.command.operation_id => {
            local
        }
        (Some(root), _)
            if request
                .as_ref()
                .is_some_and(|r| r.operation_id == root.command.operation_id) =>
        {
            root
        }
        (_, Some(local)) => local,
        _ => return None,
    };
    Some(UpdateReport {
        operation_id: state.command.operation_id,
        state: if state.phase == "rolling_back" {
            "restarting".into()
        } else {
            state.phase
        },
        detail: state.detail,
    })
}
pub fn mark_healthy(state_dir: &str) -> anyhow::Result<()> {
    let root: State = match read_bounded(&Path::new(ROOT_STATE).join("status.json"), 8192)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
    {
        Some(state) => state,
        None => return Ok(()),
    };
    if (root.command.version == version() && root.phase == "restarting")
        || root.phase == "rolling_back"
    {
        atomic_write(
            &Path::new(state_dir).join("update/healthy.json"),
            &serde_json::to_vec(&(root.command.operation_id, &root.phase))?,
        )?;
    }
    Ok(())
}
async fn restart() -> anyhow::Result<()> {
    let status = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new("systemctl")
            .args(["restart", "hope-agent.service"])
            .kill_on_drop(true)
            .status(),
    )
    .await??;
    anyhow::ensure!(status.success(), "agent service restart failed");
    Ok(())
}
fn install_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    crate::outbox::atomic_write_mode(path, bytes, 0o755)?;
    Ok(())
}
/// Invoked only by the root-owned unit. Input paths and executable destination are fixed.
pub async fn apply() -> anyhow::Result<()> {
    let input = Path::new(crate::config::DEFAULT_STATE_DIR).join("update");
    let root = Path::new(ROOT_STATE);
    let previous: Option<State> = read_bounded(&root.join("status.json"), 8192)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok());
    // A helper interrupted during replacement must recover before accepting another request.
    if let Some(mut previous) = previous.clone()
        && ["installing", "restarting", "rolling_back"].contains(&previous.phase.as_str())
    {
        return rollback(
            root,
            &mut previous,
            "Updater interrupted; restored previous binary",
        )
        .await;
    }
    let command: UpdateCommand = match read_bounded(&input.join("request.json"), 4096) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(_) => return Ok(()),
    };
    command.validate()?;
    if previous
        .as_ref()
        .is_some_and(|state| state.command.operation_id == command.operation_id)
    {
        return Ok(());
    }
    let mut state = State {
        command,
        phase: "verifying".into(),
        detail: "Verifying staged release".into(),
    };
    let result = async {
        let manifest = read_bounded(&input.join("manifest.json"), 512 * 1024)?;
        let signature = read_bounded(&input.join("manifest.json.sig"), 4096)?;
        let binary = read_bounded(&input.join("agent"), 64 * 1024 * 1024)?;
        let artifact = crate::release_verify::verify_release_manifest(
            &manifest,
            std::str::from_utf8(&signature)?.trim(),
            &binary,
        )?;
        anyhow::ensure!(
            artifact.record.version == state.command.version,
            "requested release mismatch"
        );
        let previous = read_bounded(Path::new(AGENT_PATH), 64 * 1024 * 1024)?;
        install_bytes(&root.join("previous-agent"), &previous)?;
        state.phase = "installing".into();
        save_state(root, &state)?;
        install_bytes(Path::new(AGENT_PATH), &binary)?;
        state.phase = "restarting".into();
        state.detail = "Waiting for target agent to resume local collection".into();
        save_state(root, &state)?;
        restart().await?;
        let deadline = tokio::time::Instant::now()
            + Duration::from_secs(u64::from(state.command.deadline_seconds));
        while tokio::time::Instant::now() < deadline {
            let healthy: Option<(uuid::Uuid, String)> =
                read_bounded(&input.join("healthy.json"), 256)
                    .ok()
                    .and_then(|data| serde_json::from_slice(&data).ok());
            if healthy == Some((state.command.operation_id, "restarting".into())) {
                state.phase = "awaiting_health".into();
                state.detail = "Local collection resumed; awaiting server confirmation".into();
                save_state(root, &state)?;
                // Future updates use the target release's embedded rotation keys.
                install_bytes(Path::new(HELPER_PATH), &binary)?;
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        anyhow::bail!("Target agent did not resume local collection before health deadline");
    }
    .await;
    if let Err(error) = result {
        if ["installing", "restarting"].contains(&state.phase.as_str()) {
            rollback(root, &mut state, &error.to_string()).await?;
        } else {
            state.phase = "failed".into();
            state.detail = error.to_string().chars().take(400).collect();
            save_state(root, &state)?;
        }
    }
    Ok(())
}
async fn rollback(root: &Path, state: &mut State, reason: &str) -> anyhow::Result<()> {
    state.phase = "rolling_back".into();
    state.detail = format!("Restoring previous binary: {reason}");
    save_state(root, state)?;
    let result = async {
        install_bytes(
            Path::new(AGENT_PATH),
            &read_bounded(&root.join("previous-agent"), 64 * 1024 * 1024)?,
        )?;
        restart().await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while tokio::time::Instant::now() < deadline {
            let healthy: Option<(uuid::Uuid, String)> = read_bounded(
                &Path::new(crate::config::DEFAULT_STATE_DIR).join("update/healthy.json"),
                256,
            )
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
            if healthy == Some((state.command.operation_id, "rolling_back".into())) {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        anyhow::bail!("restored agent did not resume local collection")
    }
    .await;
    state.phase = if result.is_ok() {
        "rolled_back"
    } else {
        "failed"
    }
    .into();
    state.detail = format!(
        "{reason}; {}",
        result
            .err()
            .map_or("previous service restored".into(), |error| format!(
                "rollback failed: {error}"
            ))
    )
    .chars()
    .take(500)
    .collect();
    save_state(root, state)
}
