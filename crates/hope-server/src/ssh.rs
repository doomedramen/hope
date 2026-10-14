//! SSH installation jobs.
//!
//! Trust model: the host key is discovered in one request and confirmed by the user; the install
//! job is bound to that address, port and fingerprint and aborts inside `check_server_key`, before
//! any authentication material is sent. Remote output is untrusted: only a strict, allowlisted
//! progress grammar is interpreted, user-facing text always comes from the server-side catalog,
//! and nothing the remote sends is stored or logged.
use crate::{
    agents,
    error::{ApiError, ApiResult},
    state::{State, hash, now},
};
use axum::{
    Json,
    extract::{Path, State as Extract},
};
use hope_protocol::{
    FingerprintResponse, Host, InstallJob, InstallMode, InstallStage, InstallStatus,
};
use russh::{
    ChannelMsg, client,
    keys::{PrivateKeyWithHashAlg, ssh_key},
};
use sqlx::Row;
use std::{
    net::IpAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::Mutex,
    time::{Instant, sleep, timeout, timeout_at},
};
use uuid::Uuid;
use zeroize::Zeroizing;

pub const SERVER_RESTARTED_MESSAGE: &str = "The server stopped or restarted while this installation was running. The remote host's state is unknown: it may be partly installed, and its agent may still enroll later. Inspect the host before retrying.";
const MAX_CONCURRENT_JOBS: usize = 4;
const MAX_HISTORY: i64 = 100;
const HISTORY_SECONDS: f64 = 30.0 * 86400.0;

/// Bounds for connection setup, remote silence, total lifetime and untrusted output.
#[derive(Debug, Clone)]
pub struct Limits {
    pub connect: Duration,
    /// Maximum time without any received output bytes (reset by every byte).
    pub inactivity: Duration,
    /// Maximum time from job start to a terminated installer.
    pub total: Duration,
    /// How long to wait for server-side evidence of an accepted report after the installer exits 0.
    pub report_wait: Duration,
    pub max_line: usize,
    pub max_bytes: usize,
    /// Test hook: make the job task panic, to prove the slot and job state are recovered.
    #[doc(hidden)]
    pub panic_for_test: bool,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            inactivity: Duration::from_secs(300),
            total: Duration::from_secs(1200),
            report_wait: Duration::from_secs(120),
            max_line: 1024,
            max_bytes: 1024 * 1024,
            panic_for_test: false,
        }
    }
}
pub fn set_limits(state: &State, limits: Limits) {
    if let Ok(mut current) = state.0.ssh_limits.lock() {
        *current = limits;
    }
}
fn limits(state: &State) -> Limits {
    state
        .0
        .ssh_limits
        .lock()
        .map(|l| l.clone())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// Vetted catalog: codes accepted from the remote installer or produced by the server.
// ---------------------------------------------------------------------------------------------

struct CodeInfo {
    code: &'static str,
    /// Remote state cannot be determined from what the server observed.
    uncertain: bool,
    message: &'static str,
    guidance: &'static str,
}
const fn info(
    code: &'static str,
    uncertain: bool,
    message: &'static str,
    guidance: &'static str,
) -> CodeInfo {
    CodeInfo {
        code,
        uncertain,
        message,
        guidance,
    }
}
/// Codes the installer script may emit via `hope-install: error=CODE`.
const REMOTE_CODES: &[CodeInfo] = &[
    info(
        "UNSUPPORTED_PLATFORM",
        false,
        "The host operating system is not supported.",
        "Agents run on Linux and macOS. Choose a supported host.",
    ),
    info(
        "UNSUPPORTED_ARCH",
        false,
        "The host CPU architecture is not supported.",
        "Agents are packaged for x86_64 and ARM64 only.",
    ),
    info(
        "NO_SERVICE_MANAGER",
        false,
        "No usable service manager was found on the host.",
        "Use a host with running systemd (or launchd on macOS). For a user service, log in normally so a user manager exists, then retry.",
    ),
    info(
        "SUDO_UNAVAILABLE",
        false,
        "A system installation needs root or passwordless sudo, which is unavailable.",
        "Connect as root, allow passwordless sudo for this account, or retry as an ordinary user to get a user service.",
    ),
    info(
        "EXISTING_INSTALL_MODE",
        false,
        "A conflicting system or user installation already exists on the host.",
        "Remove or complete the other installation on the host, then retry.",
    ),
    info(
        "NOT_INSTALLED",
        false,
        "No existing installation was found to operate on.",
        "Run a fresh installation instead.",
    ),
    info(
        "INVALID_MODE",
        false,
        "The requested installation mode is not allowed for this account.",
        "Retry as a user that can create the intended kind of service.",
    ),
    info(
        "INSECURE_URL",
        false,
        "The server URL is not HTTPS, which the installer requires.",
        "Ask the administrator to configure an https:// canonical server URL, then retry.",
    ),
    info(
        "MALFORMED_URL",
        false,
        "The server URL is not in a form the installer accepts.",
        "Ask the administrator to correct the canonical server URL, then retry.",
    ),
    info(
        "CA_UNREADABLE",
        false,
        "The custom CA certificate is missing or unreadable on the host.",
        "Place the CA certificate at the configured agent CA path on the remote host, make it readable by this account, then retry.",
    ),
    info(
        "PATH_UNWRITABLE",
        false,
        "The installer cannot write to a required location on the host.",
        "Check permissions and symbolic links for the agent directories, or retry with a privileged account.",
    ),
    info(
        "INVALID_SETTING",
        false,
        "An installer setting was rejected.",
        "Retry; if it persists, ask the administrator to check the installer configuration.",
    ),
    info(
        "USAGE",
        false,
        "The installer rejected its invocation.",
        "Ask the administrator to check that the server and installer versions match.",
    ),
    info(
        "RELEASE_UNAVAILABLE",
        false,
        "The server has no signed release manifest listing an agent for this host's platform.",
        "Ask the administrator to publish a signed release (scripts/assemble-release.sh, or scripts/package-agent.sh for development) that includes this operating system and architecture, then retry.",
    ),
    info(
        "DOWNLOAD_FAILED",
        false,
        "The host could not download from the H.O.P.E server.",
        "Check that the host can reach the server URL over HTTPS and trusts its certificate (or custom CA), then retry.",
    ),
    info(
        "BAD_SIGNATURE",
        false,
        "The downloaded agent failed signature verification and was not run.",
        "Do not retry until an administrator confirms the release signing key and packages.",
    ),
    info(
        "MANIFEST_INVALID",
        false,
        "The signed release manifest failed verification or does not match the downloaded agent, so nothing was run.",
        "Do not retry until an administrator re-publishes the release with scripts/assemble-release.sh and confirms the signing key.",
    ),
    info(
        "CHECKSUM_MISMATCH",
        false,
        "The downloaded agent does not match the checksum or size in the signed manifest and was not run.",
        "Retry once in case of a damaged download; if it repeats, ask the administrator to re-publish the release.",
    ),
    info(
        "DOWNGRADE_REFUSED",
        false,
        "The server's release is older than the agent already installed on the host.",
        "Publish a newer release, or run the installer with HOPE_ALLOW_DOWNGRADE=1 on the host if a downgrade is intended.",
    ),
    info(
        "BINARY_INVALID",
        false,
        "The installed agent did not run on the host.",
        "Check that the packaged release matches the host's platform and libc, then retry.",
    ),
    info(
        "BINARY_NOT_EXECUTABLE",
        false,
        "The host cannot execute programs from its temporary directory.",
        "Remount the temporary directory without noexec, or give this account an executable TMPDIR, then retry.",
    ),
    info(
        "EXISTING_IDENTITY_MISMATCH",
        false,
        "The host already runs an agent with a different identity.",
        "Reinstall from the host record that owns that agent, or uninstall the existing agent first.",
    ),
    info(
        "SERVICE_ACCOUNT_INVALID",
        false,
        "A pre-existing service account does not meet the installer's safety requirements.",
        "Remove or repair the agent service account on the host, then retry.",
    ),
    info(
        "CONFIGURE_FAILED",
        false,
        "The agent could not be configured or enrolled.",
        "Check that the host reaches the server over HTTPS; the enrollment token lasts 15 minutes and is single use, so start a new job.",
    ),
    info(
        "SERVICE_START_FAILED",
        false,
        "The agent service did not start on the host.",
        "Inspect the service manager's logs on the host, then retry.",
    ),
    info(
        "FIRST_REPORT_TIMEOUT",
        false,
        "The agent started but the server did not receive its first report in time.",
        "Check that the host can reach the server URL, trusts its certificate or custom CA, and that its clock is correct, then retry.",
    ),
    info(
        "INTERRUPTED",
        true,
        "The installer was interrupted on the host.",
        "Inspect the host to see which installation is active before retrying.",
    ),
    info(
        "ROLLED_BACK",
        false,
        "The installer failed and reported restoring the previous installation.",
        "Check the host before retrying; the previous service should be active again.",
    ),
    info(
        "ROLLBACK_INCOMPLETE",
        true,
        "The installer could not fully restore the previous installation.",
        "Log in to the host and inspect or repair the agent service manually before retrying.",
    ),
];
/// Codes the server itself produces (never taken from remote text).
const SERVER_CODES: &[CodeInfo] = &[
    info(
        "HOST_KEY_MISMATCH",
        false,
        "The host key differs from the fingerprint you confirmed. No credentials were sent.",
        "Inspect the host fingerprint again and verify it through a trusted channel. A changed key can mean a reinstall or an interception.",
    ),
    info(
        "CONNECT_FAILED",
        false,
        "The server could not establish an SSH session with the host.",
        "Check the address, port and network path, then retry.",
    ),
    info(
        "AUTH_FAILED",
        false,
        "The host rejected the credentials. Nothing was installed.",
        "Check the username and password or key, then start a new job.",
    ),
    info(
        "KEY_INVALID",
        false,
        "The private key could not be read or decrypted. No connection was made.",
        "Check the key format and passphrase, then start a new job.",
    ),
    info(
        "CONNECTION_LOST",
        true,
        "The SSH connection ended before the installer reported a result. Remote state is unknown.",
        "Inspect the host to see whether the agent was installed or the previous installation was kept, then retry.",
    ),
    info(
        "INACTIVITY_TIMEOUT",
        true,
        "The installer produced no output for too long. Remote state is unknown.",
        "Inspect the host, which may still be running the installer, before retrying.",
    ),
    info(
        "TOTAL_TIMEOUT",
        true,
        "The installation exceeded its time limit. Remote state is unknown.",
        "Inspect the host, which may still be running the installer, before retrying.",
    ),
    info(
        "OUTPUT_LIMIT",
        true,
        "The host sent more output than the server accepts, so the session was closed. Remote state is unknown.",
        "Inspect the host before retrying.",
    ),
    info(
        "INSTALLER_FAILED",
        true,
        "The installer exited with an error but gave no recognized reason. Remote state is unknown.",
        "Inspect the host, or run the manual installation command there to see the full output.",
    ),
    info(
        "INSTALLER_INCOMPLETE",
        true,
        "The installer ended without confirming completion. Remote state is unknown.",
        "Inspect the host, or run the manual installation command there to see the full output.",
    ),
    info(
        "IDENTITY_MISMATCH",
        false,
        "The host enrolled with a different identity than the one this job was bound to.",
        "Check which agent runs on the host and reconcile the host records before retrying.",
    ),
    info(
        "TOKEN_FAILED",
        false,
        "The server could not prepare an enrollment token.",
        "Retry; check the server logs if it persists.",
    ),
    info(
        "INTERNAL_ERROR",
        true,
        "The server hit an internal error while running this job. Remote state is unknown.",
        "Inspect the host before retrying, and check the server logs.",
    ),
    info(
        "HOST_ASSOCIATION_UNCONFIRMED",
        true,
        "An agent enrolled and reported, but it could not be matched to the selected host. The installation may have succeeded.",
        "Check the host list for the new agent and merge it with the selected host if it is the same machine.",
    ),
    info(
        "SERVER_RESTARTED",
        true,
        SERVER_RESTARTED_MESSAGE,
        "Inspect the host, then start a new installation job.",
    ),
];
fn lookup(code: &str) -> Option<&'static CodeInfo> {
    SERVER_CODES
        .iter()
        .chain(REMOTE_CODES)
        .find(|c| c.code == code)
}
fn remote_code(code: &[u8]) -> Option<&'static str> {
    REMOTE_CODES
        .iter()
        .find(|c| c.code.as_bytes() == code)
        .map(|c| c.code)
}

// ---------------------------------------------------------------------------------------------
// Strict progress grammar.
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Phase(InstallStage),
    Error(&'static str),
    Notice(&'static str),
}
const PHASES: &[(&str, InstallStage)] = &[
    ("preflight", InstallStage::Preflight),
    ("recover", InstallStage::Recover),
    ("uninstall", InstallStage::Uninstall),
    ("stage", InstallStage::Stage),
    ("verify", InstallStage::Verify),
    ("stop", InstallStage::Stop),
    ("place-binary", InstallStage::PlaceBinary),
    ("configure", InstallStage::Configure),
    ("write-unit", InstallStage::WriteUnit),
    ("activate", InstallStage::Activate),
    ("wait-report", InstallStage::WaitReport),
    ("rollback", InstallStage::Rollback),
    ("complete", InstallStage::Complete),
];
/// Exact installer notices mapped to vetted text; anything else after `notice=` is dropped.
const NOTICES: &[(&str, &str)] = &[
    (
        "Non-interactive sudo is unavailable, so a user service will be installed instead of a system service.",
        "Passwordless sudo is unavailable on the host, so a user service is being installed instead of a system service.",
    ),
    (
        "Using non-interactive sudo for a system service.",
        "Installing a system service with passwordless sudo.",
    ),
    (
        "Installing a user service. Reporting may stop after logout (systemd lingering) or outside a macOS GUI session.",
        "Installing a user service. Reporting may stop after logout (systemd lingering) or outside a macOS GUI session.",
    ),
];
/// Interpret one output line. Only `hope-install: phase=<known>`, `hope-install: error=<KNOWN>`
/// (optionally followed by a space and discarded text) and exact known notices are recognized.
pub fn parse_line(line: &[u8]) -> Option<Event> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let rest = line.strip_prefix(b"hope-install: ")?;
    if let Some(name) = rest.strip_prefix(b"phase=") {
        return PHASES
            .iter()
            .find(|(n, _)| n.as_bytes() == name)
            .map(|(_, s)| Event::Phase(*s));
    }
    if let Some(tail) = rest.strip_prefix(b"error=") {
        // Only the code is interpreted; whatever follows the first space is ignored, however
        // long or malformed.
        let end = tail.iter().position(|b| *b == b' ').unwrap_or(tail.len());
        return remote_code(&tail[..end]).map(Event::Error);
    }
    if let Some(text) = rest.strip_prefix(b"notice=") {
        return NOTICES
            .iter()
            .find(|(raw, _)| raw.as_bytes() == text)
            .map(|(_, vetted)| Event::Notice(vetted));
    }
    None
}
/// Splits a byte stream into lines, keeping at most `max` leading bytes of each; the remainder of
/// a longer line is discarded unread (only the prefix is ever interpreted).
struct Lines {
    buffer: Vec<u8>,
    max: usize,
}
impl Lines {
    fn new(max: usize) -> Self {
        Self {
            buffer: vec![],
            max,
        }
    }
    fn push(&mut self, data: &[u8], mut line: impl FnMut(&[u8])) {
        for byte in data {
            if *byte == b'\n' {
                line(&self.buffer);
                self.buffer.clear();
            } else if self.buffer.len() < self.max {
                self.buffer.push(*byte);
            }
        }
    }
    fn finish(&mut self, mut line: impl FnMut(&[u8])) {
        if !self.buffer.is_empty() {
            line(&self.buffer);
        }
        self.buffer.clear();
    }
}

// ---------------------------------------------------------------------------------------------
// Host-key pinning.
// ---------------------------------------------------------------------------------------------

struct PinnedClient {
    expected: Option<String>,
    observed: Arc<Mutex<Option<String>>>,
    mismatch: Arc<AtomicBool>,
}
impl client::Handler for PinnedClient {
    type Error = russh::Error;
    async fn check_server_key(&mut self, key: &ssh_key::PublicKey) -> Result<bool, Self::Error> {
        let fingerprint = key.fingerprint(ssh_key::HashAlg::Sha256).to_string();
        *self.observed.lock().await = Some(fingerprint.clone());
        // Key exchange precedes authentication, so refusing here sends no credentials. A
        // fingerprint-only inspection (expected = None) never authenticates or runs commands.
        let trusted = self
            .expected
            .as_ref()
            .is_none_or(|expected| *expected == fingerprint);
        if !trusted {
            self.mismatch.store(true, Ordering::SeqCst);
        }
        Ok(trusted)
    }
}
pub use hope_protocol::SshTarget as Target;
async fn validate_target(state: &State, target: &Target) -> ApiResult<(IpAddr, u16, Host)> {
    let host = state
        .host(&target.host_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !host.addresses.contains(&target.address) {
        return Err(ApiError::bad("Choose an address belonging to this host"));
    }
    let ip = target
        .address
        .parse()
        .map_err(|_| ApiError::bad("SSH requires an IP address"))?;
    let port = target.port.unwrap_or(22);
    if port == 0 {
        return Err(ApiError::bad("Choose a valid SSH port"));
    }
    Ok((ip, port, host))
}
fn config() -> Arc<client::Config> {
    // Silence and lifetime are enforced by the job loop, which distinguishes them from a lost
    // connection; the transport itself must not close a legitimately quiet session.
    Arc::new(client::Config {
        inactivity_timeout: None,
        ..Default::default()
    })
}
pub async fn fingerprint(
    Extract(state): Extract<State>,
    Json(target): Json<Target>,
) -> ApiResult<Json<FingerprintResponse>> {
    let (ip, port, _) = validate_target(&state, &target).await?;
    let observed = Arc::new(Mutex::new(None));
    let session = timeout(
        limits(&state).connect,
        client::connect(
            config(),
            (ip, port),
            PinnedClient {
                expected: None,
                observed: observed.clone(),
                mismatch: Arc::default(),
            },
        ),
    )
    .await
    .map_err(|_| ApiError::bad("SSH connection timed out"))?
    .map_err(|_| ApiError::bad("Could not negotiate SSH with this host"))?;
    let fingerprint = observed
        .lock()
        .await
        .clone()
        .ok_or_else(|| ApiError::bad("No SSH host key received"))?;
    let _ = session
        .disconnect(
            russh::Disconnect::ByApplication,
            "Fingerprint inspection complete",
            "en",
        )
        .await;
    Ok(Json(FingerprintResponse {
        fingerprint,
        instruction: "Verify this fingerprint through a trusted channel before installation."
            .into(),
    }))
}

// ---------------------------------------------------------------------------------------------
// Job persistence (nonsecret metadata only).
// ---------------------------------------------------------------------------------------------

fn stage_name(stage: InstallStage) -> String {
    serde_json::to_value(stage)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}
fn row_to_job(row: &sqlx::sqlite::SqliteRow) -> InstallJob {
    let parse = |column: &str| -> Option<serde_json::Value> {
        row.get::<Option<String>, _>(column)
            .map(serde_json::Value::String)
    };
    let error_code: Option<String> = row.get("error_code");
    InstallJob {
        id: row.get("id"),
        host: row.get("address"),
        host_id: row.get("host_id"),
        port: row.get::<i64, _>("port") as u16,
        fingerprint: row.get("fingerprint"),
        username: row.get("username"),
        mode: serde_json::from_value(serde_json::Value::String(row.get("mode")))
            .unwrap_or(InstallMode::Install),
        status: serde_json::from_value(serde_json::Value::String(row.get("status")))
            .unwrap_or(InstallStatus::Uncertain),
        stage: parse("stage").and_then(|v| serde_json::from_value(v).ok()),
        stages: serde_json::from_str(&row.get::<String, _>("stages")).unwrap_or_default(),
        guidance: error_code
            .as_deref()
            .and_then(lookup)
            .map(|c| c.guidance.to_string()),
        error_code,
        message: row.get("message"),
        notice: row.get("notice"),
        agent_id: row.get("agent_id"),
        created_at: row.get("created_at"),
        finished_at: row.get("finished_at"),
    }
}
pub async fn load_job(state: &State, id: &str) -> ApiResult<Option<InstallJob>> {
    Ok(sqlx::query("SELECT * FROM install_jobs WHERE id=?")
        .bind(id)
        .fetch_optional(&state.0.db)
        .await?
        .map(|r| row_to_job(&r)))
}
pub async fn jobs(Extract(state): Extract<State>) -> ApiResult<Json<Vec<InstallJob>>> {
    let rows = sqlx::query("SELECT * FROM install_jobs ORDER BY created_at DESC LIMIT ?")
        .bind(MAX_HISTORY)
        .fetch_all(&state.0.db)
        .await?;
    Ok(Json(rows.iter().map(row_to_job).collect()))
}
pub async fn job(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<InstallJob>> {
    load_job(&state, &id)
        .await?
        .map(Json)
        .ok_or_else(ApiError::not_found)
}

struct Outcome {
    status: InstallStatus,
    code: Option<&'static str>,
    message: String,
    agent_id: Option<String>,
}
impl Outcome {
    /// The server is stopping: the remote outcome is unknown, so never claim a result.
    fn interrupted() -> Self {
        Self {
            status: InstallStatus::Interrupted,
            code: Some("SERVER_RESTARTED"),
            message: SERVER_RESTARTED_MESSAGE.into(),
            agent_id: None,
        }
    }
    fn code(code: &'static str) -> Self {
        let entry = lookup(code);
        Self {
            status: if entry.is_some_and(|c| c.uncertain) {
                InstallStatus::Uncertain
            } else {
                InstallStatus::Failed
            },
            code: Some(code),
            message: entry
                .map(|c| c.message)
                .unwrap_or("Installation failed.")
                .into(),
            agent_id: None,
        }
    }
}
async fn record_stage(
    state: &State,
    id: &str,
    stages: &mut Vec<InstallStage>,
    stage: InstallStage,
) {
    if stages.last() != Some(&stage) {
        stages.push(stage);
    }
    let encoded = serde_json::to_string(stages).unwrap_or_else(|_| "[]".into());
    let _ = sqlx::query("UPDATE install_jobs SET stage=?,stages=? WHERE id=? AND status='running'")
        .bind(stage_name(stage))
        .bind(encoded)
        .bind(id)
        .execute(&state.0.db)
        .await;
    state.changed();
}
async fn finish(state: &State, id: &str, outcome: &Outcome) {
    let status = serde_json::to_value(outcome.status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "uncertain".into());
    for attempt in 0..3 {
        let result = sqlx::query("UPDATE install_jobs SET status=?,error_code=?,message=?,agent_id=COALESCE(?,agent_id),finished_at=? WHERE id=? AND status='running'")
            .bind(&status)
            .bind(outcome.code)
            .bind(&outcome.message)
            .bind(outcome.agent_id.as_deref())
            .bind(now())
            .bind(id)
            .execute(&state.0.db)
            .await;
        match result {
            Ok(_) => {
                tracing::info!(job=%id, status=%status, code=outcome.code.unwrap_or("none"), "SSH installation job finished");
                state.changed();
                return;
            }
            Err(error) => {
                tracing::error!(job=%id, attempt, %error, "could not record SSH job outcome");
                sleep(Duration::from_millis(200)).await;
            }
        }
    }
    // Last resort: never leave the job claiming to run.
    let fallback = Outcome::code("INTERNAL_ERROR");
    let _ = sqlx::query("UPDATE install_jobs SET status='uncertain',error_code='INTERNAL_ERROR',message=?,finished_at=? WHERE id=? AND status='running'")
        .bind(fallback.message)
        .bind(now())
        .bind(id)
        .execute(&state.0.db)
        .await;
    state.changed();
}

// ---------------------------------------------------------------------------------------------
// Starting a job.
// ---------------------------------------------------------------------------------------------

pub use hope_protocol::SshInstallRequest as InstallRequest;
struct Secrets {
    password: Option<Zeroizing<String>>,
    private_key: Option<Zeroizing<String>>,
    passphrase: Option<Zeroizing<String>>,
}
struct Plan {
    id: String,
    host_id: String,
    address: String,
    ip: IpAddr,
    port: u16,
    fingerprint: String,
    username: String,
    mode: InstallMode,
    bound_agent: Option<String>,
}

pub async fn install(
    Extract(state): Extract<State>,
    Json(request): Json<InstallRequest>,
) -> ApiResult<Json<InstallJob>> {
    if state.0.shutdown.is_cancelled() {
        return Err(ApiError::bad("The server is shutting down"));
    }
    let (ip, port, host) = validate_target(&state, &request.target).await?;
    if request.username.is_empty()
        || request.username.len() > 128
        || !request.fingerprint.starts_with("SHA256:")
        || request.fingerprint.len() > 100
    {
        return Err(ApiError::bad(
            "Username and verified SHA256 fingerprint are required",
        ));
    }
    let wrap = |value: Option<String>| value.filter(|s| !s.is_empty()).map(Zeroizing::new);
    let secrets = Secrets {
        password: wrap(request.password),
        private_key: wrap(request.private_key),
        passphrase: wrap(request.passphrase),
    };
    if secrets.password.is_none() && secrets.private_key.is_none() {
        return Err(ApiError::bad("Provide a password or private key"));
    }
    let (mode, bound_agent) = match host.agent_id {
        Some(agent) => (InstallMode::Reinstall, Some(agent)),
        None => (InstallMode::Install, None),
    };
    let id = Uuid::new_v4().to_string();
    {
        let mut running = state.0.installs.lock().await;
        if running.len() >= MAX_CONCURRENT_JOBS {
            return Err(ApiError::bad(
                "At most four SSH installations may run concurrently",
            ));
        }
        let busy: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM install_jobs WHERE status='running' AND (host_id=? OR (address=? AND port=?))",
        )
        .bind(&request.target.host_id)
        .bind(&request.target.address)
        .bind(i64::from(port))
        .fetch_one(&state.0.db)
        .await?;
        if busy > 0 {
            return Err(ApiError::bad(
                "An installation is already running for this host or SSH address; wait for it to finish",
            ));
        }
        sqlx::query("DELETE FROM install_jobs WHERE status<>'running' AND created_at<?")
            .bind(now() - HISTORY_SECONDS)
            .execute(&state.0.db)
            .await?;
        sqlx::query("DELETE FROM install_jobs WHERE status<>'running' AND id NOT IN (SELECT id FROM install_jobs ORDER BY created_at DESC LIMIT ?)")
            .bind(MAX_HISTORY - 1)
            .execute(&state.0.db)
            .await?;
        sqlx::query("INSERT INTO install_jobs(id,host_id,address,port,fingerprint,username,mode,status,stage,stages,bound_agent_id,agent_id,created_at) VALUES(?,?,?,?,?,?,?,'running','connecting','[\"connecting\"]',?,?,?)")
            .bind(&id)
            .bind(&request.target.host_id)
            .bind(&request.target.address)
            .bind(i64::from(port))
            .bind(&request.fingerprint)
            .bind(&request.username)
            .bind(if mode == InstallMode::Reinstall { "reinstall" } else { "install" })
            .bind(bound_agent.as_deref())
            .bind(bound_agent.as_deref())
            .bind(now())
            .execute(&state.0.db)
            .await?;
        // Shutdown may have begun since the first check; settle only flips rows it can see, and
        // this row is now visible, so resolve it here instead of spawning work.
        if state.0.shutdown.is_cancelled() {
            let outcome = Outcome::interrupted();
            sqlx::query("UPDATE install_jobs SET status='interrupted',error_code='SERVER_RESTARTED',message=?,finished_at=? WHERE id=?")
                .bind(&outcome.message)
                .bind(now())
                .bind(&id)
                .execute(&state.0.db)
                .await?;
            return Err(ApiError::bad("The server is shutting down"));
        }
        running.insert(id.clone());
    }
    let plan = Arc::new(Plan {
        id: id.clone(),
        host_id: request.target.host_id.clone(),
        address: request.target.address.clone(),
        ip,
        port,
        fingerprint: request.fingerprint,
        username: request.username,
        mode,
        bound_agent,
    });
    let task_state = state.clone();
    tokio::spawn(async move {
        let limits = limits(&task_state);
        let exec_started = Arc::new(AtomicBool::new(false));
        let hard = limits.total + limits.report_wait + limits.connect + Duration::from_secs(30);
        // The work runs in its own task so a panic surfaces here as a JoinError instead of
        // leaving the job running and its concurrency slot occupied forever.
        let mut worker = {
            let (state, plan, exec_started) =
                (task_state.clone(), plan.clone(), exec_started.clone());
            tokio::spawn(async move {
                timeout(
                    hard,
                    provision(&state, &plan, secrets, &limits, exec_started),
                )
                .await
            })
        };
        let started = exec_started.clone();
        let joined = tokio::select! {
            joined = &mut worker => Some(joined),
            _ = task_state.0.shutdown.cancelled() => {
                // Dropping the task closes the SSH session; the remote installer's own
                // transaction decides what remains on the host.
                worker.abort();
                None
            }
        };
        let outcome = match joined {
            None => Outcome::interrupted(),
            Some(Ok(Ok(outcome))) => outcome,
            Some(Ok(Err(_))) if started.load(Ordering::SeqCst) => Outcome::code("TOTAL_TIMEOUT"),
            Some(Ok(Err(_))) => Outcome::code("CONNECT_FAILED"),
            Some(Err(_)) => {
                tracing::error!(job=%plan.id, "SSH installation task failed unexpectedly");
                Outcome::code("INTERNAL_ERROR")
            }
        };
        finish(&task_state, &plan.id, &outcome).await;
        task_state.0.installs.lock().await.remove(&plan.id);
        task_state.changed();
    });
    load_job(&state, &id)
        .await?
        .map(Json)
        .ok_or_else(ApiError::not_found)
}

// ---------------------------------------------------------------------------------------------
// Running a job.
// ---------------------------------------------------------------------------------------------

async fn provision(
    state: &State,
    plan: &Plan,
    mut secrets: Secrets,
    limits: &Limits,
    exec_started: Arc<AtomicBool>,
) -> Outcome {
    if limits.panic_for_test {
        panic!("injected test panic");
    }
    let started = Instant::now();
    let total_deadline = started + limits.total;
    let mut stages = vec![InstallStage::Connecting];
    // Decode the key before connecting so a wrong passphrase never reaches the network.
    let key = match secrets.private_key.take() {
        Some(private) => {
            let passphrase = secrets.passphrase.take();
            // The bcrypt KDF behind encrypted keys is deliberately slow; keep it off the reactor.
            let decoded = tokio::task::spawn_blocking(move || {
                russh::keys::decode_secret_key(&private, passphrase.as_deref().map(String::as_str))
            })
            .await;
            match decoded {
                Ok(Ok(key)) => Some(key),
                _ => return Outcome::code("KEY_INVALID"),
            }
        }
        None => None,
    };
    secrets.passphrase = None;
    let mismatch = Arc::new(AtomicBool::new(false));
    let connected = timeout(
        limits.connect,
        client::connect(
            config(),
            (plan.ip, plan.port),
            PinnedClient {
                expected: Some(plan.fingerprint.clone()),
                observed: Arc::new(Mutex::new(None)),
                mismatch: mismatch.clone(),
            },
        ),
    )
    .await;
    let mut session = match connected {
        Ok(Ok(session)) => session,
        _ if mismatch.load(Ordering::SeqCst) => return Outcome::code("HOST_KEY_MISMATCH"),
        _ => return Outcome::code("CONNECT_FAILED"),
    };
    record_stage(state, &plan.id, &mut stages, InstallStage::Authenticating).await;
    let authenticated = timeout(limits.connect, async {
        if let Some(key) = key {
            let algorithm = session.best_supported_rsa_hash().await?.flatten();
            session
                .authenticate_publickey(
                    &plan.username,
                    PrivateKeyWithHashAlg::new(Arc::new(key), algorithm),
                )
                .await
                .map(|r| r.success())
        } else {
            let password = secrets.password.take().unwrap_or_default();
            session
                .authenticate_password(&plan.username, password.as_str())
                .await
                .map(|r| r.success())
        }
    })
    .await;
    // Credentials are dropped (and zeroized) as soon as authentication ends.
    drop(secrets);
    match authenticated {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => return Outcome::code("AUTH_FAILED"),
        _ => return Outcome::code("CONNECT_FAILED"),
    }
    let reset = match plan.bound_agent.as_deref() {
        Some(agent) => {
            sqlx::query_scalar::<_, bool>("SELECT revoked FROM agents WHERE id=?")
                .bind(agent)
                .fetch_optional(&state.0.db)
                .await
                .ok()
                .flatten()
                == Some(true)
        }
        None => false,
    };
    let Ok(token) = agents::issue_token_for_reset(
        state,
        reset.then_some(plan.bound_agent.as_deref()).flatten(),
    )
    .await
    else {
        return Outcome::code("TOKEN_FAILED");
    };
    let command = Zeroizing::new(agents::install_command_with_reset(state, &token, reset));
    let token_hash = hash(&token);
    drop(token);
    exec_started.store(true, Ordering::SeqCst);
    let mut channel = match session.channel_open_session().await {
        Ok(channel) => channel,
        Err(_) => return Outcome::code("CONNECT_FAILED"),
    };
    // The command (and enrollment token) travels on stdin, never in argv.
    let sent = async {
        channel.exec(true, "sh -s").await?;
        channel
            .data(format!("{}\n", command.as_str()).as_bytes())
            .await?;
        channel.eof().await
    }
    .await;
    drop(command);
    if sent.is_err() {
        return Outcome::code("CONNECTION_LOST");
    }
    let mut stdout = Lines::new(limits.max_line);
    let mut stderr = Lines::new(limits.max_line);
    let mut progress = Progress {
        stages,
        events: vec![],
        replaced_at: None,
    };
    let mut total_bytes = 0usize;
    let mut exit: Option<u32> = None;
    let mut signalled = false;
    let mut last_data = Instant::now();
    let mut ended: Option<&'static str> = None;
    loop {
        let wake = (last_data + limits.inactivity).min(total_deadline);
        match timeout_at(wake, channel.wait()).await {
            Err(_) => {
                ended = Some(if Instant::now() >= total_deadline {
                    "TOTAL_TIMEOUT"
                } else {
                    "INACTIVITY_TIMEOUT"
                });
                break;
            }
            Ok(None) => break,
            Ok(Some(ChannelMsg::ExitStatus { exit_status })) => exit = Some(exit_status),
            Ok(Some(ChannelMsg::ExitSignal { .. })) => signalled = true,
            Ok(Some(message @ (ChannelMsg::Data { .. } | ChannelMsg::ExtendedData { .. }))) => {
                last_data = Instant::now();
                let (data, lines) = match &message {
                    ChannelMsg::Data { data } => (&data[..], &mut stdout),
                    ChannelMsg::ExtendedData { data, .. } => (&data[..], &mut stderr),
                    _ => unreachable!(),
                };
                total_bytes = total_bytes.saturating_add(data.len());
                if total_bytes > limits.max_bytes {
                    ended = Some("OUTPUT_LIMIT");
                    break;
                }
                let mut batch = vec![];
                lines.push(data, |line| batch.extend(parse_line(line)));
                for event in batch {
                    apply_event(state, &plan.id, &mut progress, event).await;
                }
            }
            Ok(Some(_)) => {}
        }
    }
    if ended.is_none() {
        let mut batch = vec![];
        stdout.finish(|line| batch.extend(parse_line(line)));
        stderr.finish(|line| batch.extend(parse_line(line)));
        for event in batch {
            apply_event(state, &plan.id, &mut progress, event).await;
        }
    }
    let _ = session
        .disconnect(
            russh::Disconnect::ByApplication,
            "Installation finished",
            "en",
        )
        .await;
    if let Some(code) = ended {
        return Outcome::code(code);
    }
    let mut remote_error = None;
    let boundary = progress.replaced_at;
    let mut complete = false;
    for event in &progress.events {
        match event {
            Event::Error("ROLLBACK_INCOMPLETE") => remote_error = Some("ROLLBACK_INCOMPLETE"),
            Event::Error(code) if remote_error.is_none() => remote_error = Some(*code),
            Event::Phase(InstallStage::Complete) => complete = true,
            _ => {}
        }
    }
    let Some(status) = exit else {
        return Outcome::code(if signalled {
            "INSTALLER_INCOMPLETE"
        } else {
            "CONNECTION_LOST"
        });
    };
    if status != 0 {
        return Outcome::code(remote_error.unwrap_or("INSTALLER_FAILED"));
    }
    if !complete || remote_error.is_some() {
        return Outcome::code(remote_error.unwrap_or("INSTALLER_INCOMPLETE"));
    }
    // Without a stop/replacement phase any report could be from the previous service.
    let Some(boundary) = boundary else {
        return Outcome::code("INSTALLER_INCOMPLETE");
    };
    record_stage(
        state,
        &plan.id,
        &mut progress.stages,
        InstallStage::ConfirmingReport,
    )
    .await;
    confirm_report(state, plan, &token_hash, boundary, limits).await
}

struct Progress {
    stages: Vec<InstallStage>,
    events: Vec<Event>,
    /// When the installer first reported stopping/replacing the service (`stop` or later). The
    /// previous service is down from about then, so only later reports can come from the new one.
    replaced_at: Option<f64>,
}
async fn apply_event(state: &State, id: &str, progress: &mut Progress, event: Event) {
    match &event {
        Event::Phase(stage) => {
            if matches!(
                stage,
                InstallStage::Stop
                    | InstallStage::PlaceBinary
                    | InstallStage::Configure
                    | InstallStage::WriteUnit
                    | InstallStage::Activate
            ) {
                progress.replaced_at.get_or_insert_with(now);
            }
            record_stage(state, id, &mut progress.stages, *stage).await;
        }
        Event::Notice(text) => {
            let _ = sqlx::query("UPDATE install_jobs SET notice=? WHERE id=? AND status='running'")
                .bind(*text)
                .bind(id)
                .execute(&state.0.db)
                .await;
            state.changed();
        }
        Event::Error(_) => {}
    }
    if progress.events.len() < 256 {
        progress.events.push(event);
    }
}

enum Evidence {
    Pending,
    Confirmed(String),
    Mismatch(String),
    /// A fresh enrollment reported, but not as the selected host (reconciliation may lag or have
    /// failed), so the job cannot claim it installed on that host.
    Unassociated(String),
}
/// Server-side proof that the expected identity delivered an accepted report after activation.
/// Reinstall: the agent bound at job creation (it may keep its credential and not consume the
/// token). Fresh install: the job's token was consumed by an agent that then reported.
async fn evidence(
    state: &State,
    plan: &Plan,
    token_hash: &str,
    boundary: f64,
) -> anyhow::Result<Evidence> {
    let token: Option<(Option<f64>, Option<String>)> =
        sqlx::query_as("SELECT used_at,agent_id FROM enrollment_tokens WHERE token_hash=?")
            .bind(token_hash)
            .fetch_optional(&state.0.db)
            .await?;
    let (used_at, token_agent) = token.unwrap_or((None, None));
    let agent = match (plan.mode, plan.bound_agent.as_deref()) {
        (InstallMode::Reinstall, Some(bound)) => {
            if let Some(other) = token_agent.filter(|a| a != bound) {
                return Ok(Evidence::Mismatch(other));
            }
            bound.to_string()
        }
        _ => match (used_at, token_agent) {
            (Some(_), Some(agent)) => agent,
            _ => return Ok(Evidence::Pending),
        },
    };
    let body: Option<String> = sqlx::query_scalar(
        "SELECT h.body FROM hosts h JOIN agents a ON a.id=h.agent_id WHERE h.agent_id=? AND a.revoked=0",
    )
    .bind(&agent)
    .fetch_optional(&state.0.db)
    .await?;
    let Some(reported) = body
        .and_then(|b| serde_json::from_str::<Host>(&b).ok())
        .filter(|h| h.last_report.is_some_and(|reported| reported > boundary))
    else {
        return Ok(Evidence::Pending);
    };
    // A reinstall is bound to the host's own agent. A fresh enrollment is accepted only if it
    // became the selected host record or reports the SSH address among its own.
    let associated = plan.mode == InstallMode::Reinstall
        || reported.id == plan.host_id
        || reported.addresses.contains(&plan.address);
    Ok(if associated {
        Evidence::Confirmed(agent)
    } else {
        Evidence::Unassociated(agent)
    })
}
async fn confirm_report(
    state: &State,
    plan: &Plan,
    token_hash: &str,
    boundary: f64,
    limits: &Limits,
) -> Outcome {
    let deadline = Instant::now() + limits.report_wait;
    let poll =
        (limits.report_wait / 20).clamp(Duration::from_millis(25), Duration::from_millis(500));
    loop {
        match evidence(state, plan, token_hash, boundary).await {
            Ok(Evidence::Confirmed(agent)) => {
                return Outcome {
                    status: InstallStatus::Completed,
                    code: None,
                    message: "Agent installed and its first report was accepted by the server."
                        .into(),
                    agent_id: Some(agent),
                };
            }
            Ok(Evidence::Unassociated(agent)) => {
                let mut outcome = Outcome::code("HOST_ASSOCIATION_UNCONFIRMED");
                outcome.agent_id = Some(agent);
                return outcome;
            }
            Ok(Evidence::Mismatch(agent)) => {
                let mut outcome = Outcome::code("IDENTITY_MISMATCH");
                outcome.agent_id = Some(agent);
                return outcome;
            }
            Ok(Evidence::Pending) => {}
            Err(_) => tracing::warn!("could not check installation report evidence"),
        }
        if Instant::now() >= deadline {
            return Outcome::code("FIRST_REPORT_TIMEOUT");
        }
        sleep(poll).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_accepts_only_known_phases_errors_and_notices() {
        assert_eq!(
            parse_line(b"hope-install: phase=place-binary"),
            Some(Event::Phase(InstallStage::PlaceBinary))
        );
        assert_eq!(
            parse_line(b"hope-install: phase=complete\r"),
            Some(Event::Phase(InstallStage::Complete))
        );
        assert_eq!(
            parse_line(b"hope-install: error=RELEASE_UNAVAILABLE no package for token=abc"),
            Some(Event::Error("RELEASE_UNAVAILABLE"))
        );
        assert_eq!(
            parse_line(b"hope-install: error=ROLLED_BACK"),
            Some(Event::Error("ROLLED_BACK"))
        );
        for rejected in [
            &b"hope-install: phase=unknown"[..],
            b"hope-install: phase=complete extra",
            b"hope-install: phase=complete\x1b[31m",
            b" hope-install: phase=complete",
            b"hope-install: error=NOT_A_CODE message",
            b"hope-install: error=ROLLED_BACKX",
            b"hope-install: error=",
            b"hope-install: notice=anything else",
            b"hope-install: warning=secret-token",
            b"password=hunter2",
            b"\xff\xfehope-install: phase=complete",
        ] {
            assert_eq!(parse_line(rejected), None, "{:?}", rejected);
        }
        assert!(
            parse_line(b"hope-install: notice=Using non-interactive sudo for a system service.")
                .is_some()
        );
    }
    #[test]
    fn every_remote_code_has_a_vetted_message_and_guidance() {
        for code in REMOTE_CODES.iter().chain(SERVER_CODES) {
            assert!(!code.message.is_empty() && !code.guidance.is_empty());
        }
        let script = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/install-agent.sh"
        ))
        .expect("installer");
        for word in script.split(|c: char| !(c.is_ascii_uppercase() || c == '_')) {
            if word.len() > 5
                && (script.contains(&format!("fail {word} "))
                    || script.contains(&format!("err {word} ")))
            {
                assert!(
                    remote_code(word.as_bytes()).is_some(),
                    "{word} missing from catalog"
                );
            }
        }
    }
    #[test]
    fn lines_are_bounded_and_only_the_prefix_is_kept() {
        let mut lines = Lines::new(8);
        let mut seen = vec![];
        lines.push(b"short\nthis line is far too long\nok\npart", |l| {
            seen.push(l.to_vec())
        });
        lines.push(b"ial", |l| seen.push(l.to_vec()));
        lines.finish(|l| seen.push(l.to_vec()));
        assert_eq!(
            seen,
            vec![
                b"short".to_vec(),
                b"this lin".to_vec(),
                b"ok".to_vec(),
                b"partial".to_vec()
            ]
        );
    }
    #[test]
    fn long_or_non_utf8_tails_do_not_hide_the_code() {
        let mut line = b"hope-install: error=DOWNLOAD_FAILED ".to_vec();
        line.extend([0xff, 0xfe, 0x1b, b'x']);
        line.extend(vec![b'z'; 5000]);
        assert_eq!(parse_line(&line), Some(Event::Error("DOWNLOAD_FAILED")));
        assert_eq!(
            parse_line(b"hope-install: error=DOWNLOAD_FAILEDX tail"),
            None
        );
        assert_eq!(parse_line(b"x hope-install: error=DOWNLOAD_FAILED"), None);
    }
}
