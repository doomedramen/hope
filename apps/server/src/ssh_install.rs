//! Bounded SSH deployment and repair for the Linux agent (M6).
//!
//! This module intentionally exposes a small fixed-command surface. It does
//! not accept arbitrary shell commands, streams raw remote output to the
//! caller, or bypass host-key trust. The SSH credential is decrypted only for
//! the duration of this operation.

use std::sync::Arc;
use std::time::Duration;

use russh::ChannelMsg;
use russh::client::{self, AuthResult, Handler};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::agents;
use crate::credentials::{CredentialSecret, CredentialStore, redact_sensitive};
use crate::release_repository::ReleaseRepository;
use crate::ssh_trust::{self, HostKeyDecision};

const MAX_COMMAND_OUTPUT: usize = 64 * 1024;
const MAX_AGENT_BINARY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MAX_UPDATE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const DEFAULT_UPDATE_DEADLINE: Duration = Duration::from_secs(120);
const DEFAULT_STATE_DIR: &str = "/var/lib/hope";
const DEFAULT_SERVICE_USER: &str = "hope-agent";
const AGENT_PATH: &str = "/usr/local/libexec/hope-agent";
const SERVICE_PATH: &str = "/etc/systemd/system/hope-agent.service";

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("SSH deployment configuration is incomplete")]
    Configuration,
    #[error("SSH host is not trusted")]
    HostKeyUntrusted,
    #[error("SSH authentication failed")]
    Authentication,
    #[error("SSH connection failed")]
    Connection,
    #[error("SSH command failed")]
    Command,
    #[error("SSH command output exceeded the safety limit")]
    OutputLimit,
    #[error("SSH operation timed out")]
    Timeout,
    #[error("target is not a supported Linux host")]
    UnsupportedHost,
    #[error("agent binary is unavailable")]
    BinaryUnavailable,
    #[error("agent enrollment failed")]
    Enrollment,
    #[error("agent service did not become active")]
    ServiceUnavailable,
    #[error("agent did not check in; verify the connection address is reachable from the target")]
    FirstCheckInTimeout,
    #[error("credential vault error: {0}")]
    Credential(#[from] crate::credentials::CredentialError),
    #[error("host-key trust error: {0}")]
    Trust(#[from] ssh_trust::SshTrustError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("local file error")]
    LocalFile(#[source] std::io::Error),
}

pub type Result<T> = std::result::Result<T, InstallError>;

#[derive(Debug, Clone)]
pub struct DeploymentRequest {
    pub device_id: Uuid,
    pub host: String,
    pub port: i32,
    pub credential_id: Uuid,
    pub actor_user_id: Option<Uuid>,
    pub repair: bool,
    pub disassociate_after_enrollment: bool,
    pub connection_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeploymentResult {
    pub host: String,
    pub port: i32,
    pub platform: String,
    pub architecture: String,
    pub enrolled: bool,
    pub repaired: bool,
}

#[derive(Debug, Clone)]
pub struct AgentUpdateRequest {
    pub device_id: Uuid,
    pub agent_id: Uuid,
    pub host: String,
    pub port: i32,
    pub credential_id: Uuid,
    pub actor_user_id: Option<Uuid>,
    pub target_version: String,
    pub previous_version: Option<String>,
    pub expected_platform: String,
    pub expected_architecture: String,
    pub binary: Vec<u8>,
    pub check_in_deadline: Duration,
    pub repair_on_failure: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentUpdateState {
    Succeeded,
    RolledBack,
    Repaired,
}

#[derive(Debug, Clone)]
pub struct AgentUpdateResult {
    pub state: AgentUpdateState,
    pub platform: String,
    pub architecture: String,
}

#[derive(Debug, Clone)]
struct DeploymentConfig {
    enroll_url: String,
    gateway_url: String,
    state_dir: String,
    service_user: String,
    timeout: Duration,
}

impl DeploymentConfig {
    fn from_environment(connection_url: &str) -> Result<Self> {
        let (enroll_url, gateway_url, _) = connection_endpoints(connection_url)?;
        validate_url(&enroll_url, "https://")?;
        validate_url(&gateway_url, "wss://")?;
        let state_dir =
            std::env::var("HOPE_AGENT_STATE_DIR").unwrap_or_else(|_| DEFAULT_STATE_DIR.to_string());
        let service_user = std::env::var("HOPE_AGENT_SERVICE_USER")
            .unwrap_or_else(|_| DEFAULT_SERVICE_USER.to_string());
        validate_state_dir(&state_dir)?;
        validate_service_user(&service_user)?;
        validate_unit_argument(&gateway_url)?;
        let timeout_seconds = std::env::var("HOPE_AGENT_INSTALL_TIMEOUT_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(120)
            .clamp(10, MAX_INSTALL_TIMEOUT.as_secs());
        Ok(Self {
            enroll_url,
            gateway_url,
            state_dir,
            service_user,
            timeout: Duration::from_secs(timeout_seconds),
        })
    }
}

#[derive(Clone)]
struct HostKeyHandler {
    pool: PgPool,
    device_id: Option<Uuid>,
    host: String,
    port: i32,
    actor_user_id: Option<Uuid>,
    decision: Arc<Mutex<Option<HostKeyDecision>>>,
}

impl Handler for HostKeyHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> anyhow::Result<bool> {
        let public_key = server_public_key.public_key();
        let fingerprint = public_key.fingerprint(HashAlg::Sha256).to_string();
        let key_type = public_key.algorithm().as_str().to_string();
        let decision = ssh_trust::observe(
            &self.pool,
            self.device_id,
            &self.host,
            self.port,
            &key_type,
            &fingerprint,
            self.actor_user_id,
        )
        .await
        .map_err(|error| {
            tracing::error!(
                host = %self.host,
                port = self.port,
                key_type = %key_type,
                error = %error,
                "could not persist SSH host-key observation"
            );
            anyhow::Error::from(error)
        })?;
        let permits = decision.permits_connection();
        tracing::info!(
            host = %self.host,
            port = self.port,
            key_type = %key_type,
            fingerprint = %fingerprint,
            decision = ?decision,
            permits,
            "observed SSH host key"
        );
        *self.decision.lock().await = Some(decision);
        Ok(permits)
    }
}

pub async fn install_or_repair(
    pool: &PgPool,
    store: &CredentialStore,
    request: DeploymentRequest,
) -> Result<DeploymentResult> {
    let config = DeploymentConfig::from_environment(&request.connection_url)?;
    let host = ssh_trust::normalize_host(&request.host)?;
    ssh_trust::validate_port(request.port)?;
    let secret = store
        .decrypt_for_agent_use(
            request.credential_id,
            request.device_id,
            request.actor_user_id,
        )
        .await?;

    let (mut session, decision) = connect_authenticated(
        pool,
        Some(request.device_id),
        &host,
        request.port,
        request.actor_user_id,
        &secret,
        config.timeout,
    )
    .await?;
    if !decision
        .lock()
        .await
        .as_ref()
        .is_some_and(HostKeyDecision::permits_connection)
    {
        return Err(InstallError::HostKeyUntrusted);
    }

    let platform = run_command(&mut session, "uname -s", config.timeout).await?;
    let architecture = run_command(&mut session, "uname -m", config.timeout).await?;
    let platform = single_line(&platform.stdout).ok_or(InstallError::UnsupportedHost)?;
    let architecture = single_line(&architecture.stdout).ok_or(InstallError::UnsupportedHost)?;
    if platform != "Linux" {
        return Err(InstallError::UnsupportedHost);
    }
    if architecture != "x86_64" {
        return Err(InstallError::UnsupportedHost);
    }
    let release_arch = "amd64";
    let repository =
        ReleaseRepository::from_environment().map_err(|_| InstallError::BinaryUnavailable)?;
    let (release, artifact) = repository
        .latest_compatible_for_channel(
            "linux",
            release_arch,
            protocol::PROTOCOL_VERSION,
            release::ReleaseChannel::Stable,
        )
        .map_err(|_| InstallError::BinaryUnavailable)?;
    let binary = repository
        .read_artifact(&release, &artifact)
        .map_err(|_| InstallError::BinaryUnavailable)?;
    let sudo = privilege_prefix(&mut session, config.timeout).await?;
    let remote_upload = format!("/tmp/hope-agent-upload-{}", Uuid::new_v4().simple());
    upload_binary(&mut session, &sudo, &remote_upload, &binary, config.timeout).await?;
    run_command(
        &mut session,
        &format!(
            "{sudo}install -o root -g root -m 0755 {remote_upload} {AGENT_PATH} && {sudo}rm -f {remote_upload}"
        ),
        config.timeout,
    )
    .await?;

    prepare_service_account(&mut session, &sudo, &config, config.timeout).await?;
    let state_present = run_command(
        &mut session,
        &format!(
            "{sudo}test -s {state}/agent-identity.hex && test -s {state}/agent-id && \
             test -s {state}/tls-trust && echo enrolled || echo missing",
            state = sh_quote(&config.state_dir),
        ),
        config.timeout,
    )
    .await?
    .stdout;
    let mut enrolled = false;
    if state_present.trim() != "enrolled" {
        let token = agents::create_enrollment_token(pool, 15).await?;
        let tls_fingerprint = tls_fingerprint_from_environment()?;
        let code = format!("{token}.{tls_fingerprint}");
        let user_prefix = if sudo.is_empty() {
            format!("runuser -u {} -- ", sh_quote(&config.service_user))
        } else {
            format!("{sudo}-u {} ", sh_quote(&config.service_user))
        };
        let (_, _, system_tls) = connection_endpoints(&request.connection_url)?;
        let command = format!(
            "{user_prefix}{AGENT_PATH} enroll --server {} --code-stdin --state-dir {}{}",
            sh_quote(&config.enroll_url),
            sh_quote(&config.state_dir),
            if system_tls { " --system-tls" } else { "" },
        );
        let output = run_with_stdin(&mut session, &command, code.as_bytes(), config.timeout).await;
        if let Err(error) = output {
            // The enrollment token is never placed in the error text.
            return Err(match error {
                InstallError::Timeout => InstallError::Timeout,
                _ => InstallError::Enrollment,
            });
        }
        enrolled = true;
    }

    install_service_unit(&mut session, &sudo, &config, config.timeout).await?;
    let restart_started_at = time::OffsetDateTime::now_utc();
    run_command(
        &mut session,
        &format!("{sudo}systemctl daemon-reload && {sudo}systemctl enable --now hope-agent"),
        config.timeout,
    )
    .await?;
    let active = run_command(
        &mut session,
        &format!("{sudo}systemctl is-active --quiet hope-agent"),
        config.timeout,
    )
    .await;
    if active.is_err() {
        return Err(InstallError::ServiceUnavailable);
    }

    let agent_id_output = run_command(
        &mut session,
        &format!("{sudo}cat {}/agent-id", sh_quote(&config.state_dir)),
        config.timeout,
    )
    .await?;
    let agent_id =
        Uuid::parse_str(agent_id_output.stdout.trim()).map_err(|_| InstallError::Enrollment)?;
    if !wait_for_first_check_in(pool, agent_id, restart_started_at, Duration::from_secs(90)).await?
    {
        return Err(InstallError::FirstCheckInTimeout);
    }
    if request.disassociate_after_enrollment {
        store
            .disassociate_device(
                request.credential_id,
                request.device_id,
                request.actor_user_id,
            )
            .await?;
    }

    let _ = session
        .disconnect(russh::Disconnect::ByApplication, "done", "")
        .await;
    drop(secret);
    Ok(DeploymentResult {
        host,
        port: request.port,
        platform: platform.to_string(),
        architecture: architecture.to_string(),
        enrolled,
        repaired: request.repair,
    })
}

/// Push one already-verified release over the trusted SSH connection. The
/// remote replacement is an atomic same-filesystem rename. The old binary is
/// retained until the agent has checked in with the target version; if that
/// deadline expires, the worker restores it and restarts the service.
pub async fn update_or_rollback(
    pool: &PgPool,
    store: &CredentialStore,
    request: AgentUpdateRequest,
) -> Result<AgentUpdateResult> {
    validate_update_version(&request.target_version)?;
    if request.binary.is_empty() || request.binary.len() as u64 > MAX_AGENT_BINARY_BYTES {
        return Err(InstallError::BinaryUnavailable);
    }
    if request.expected_platform != "linux" || request.expected_architecture != "amd64" {
        return Err(InstallError::UnsupportedHost);
    }

    let connection_url = std::env::var("HOPE_AGENT_CONNECTION_URL")
        .unwrap_or_else(|_| "http://localhost".to_string());
    let config = DeploymentConfig::from_environment(&connection_url)?;
    let host = ssh_trust::normalize_host(&request.host)?;
    ssh_trust::validate_port(request.port)?;
    let secret = store
        .decrypt_for_agent_use(
            request.credential_id,
            request.device_id,
            request.actor_user_id,
        )
        .await?;
    let (mut session, decision) = connect_authenticated(
        pool,
        Some(request.device_id),
        &host,
        request.port,
        request.actor_user_id,
        &secret,
        config.timeout,
    )
    .await?;
    if !decision
        .lock()
        .await
        .as_ref()
        .is_some_and(HostKeyDecision::permits_connection)
    {
        return Err(InstallError::HostKeyUntrusted);
    }

    let platform_output = run_command(&mut session, "uname -s", config.timeout).await?;
    let platform = single_line(&platform_output.stdout).ok_or(InstallError::UnsupportedHost)?;
    let architecture_output = run_command(&mut session, "uname -m", config.timeout).await?;
    let remote_arch =
        single_line(&architecture_output.stdout).ok_or(InstallError::UnsupportedHost)?;
    let architecture = match remote_arch {
        "x86_64" => "amd64",
        _ => return Err(InstallError::UnsupportedHost),
    };
    if platform != "Linux"
        || request.expected_platform != "linux"
        || request.expected_architecture != architecture
    {
        return Err(InstallError::UnsupportedHost);
    }

    let sudo = privilege_prefix(&mut session, config.timeout).await?;
    let operation_id = Uuid::new_v4().simple().to_string();
    let remote_upload = format!("/tmp/hope-agent-update-{operation_id}");
    let staged_path = format!("{AGENT_PATH}.staged-{operation_id}");
    let rollback_path = format!("{AGENT_PATH}.rollback-{operation_id}");
    upload_binary(
        &mut session,
        &sudo,
        &remote_upload,
        &request.binary,
        config.timeout,
    )
    .await?;

    let install_and_restart = format!(
        "{sudo}install -o root -g root -m 0755 {remote_upload} {staged_path} && \
         {sudo}cp -p {AGENT_PATH} {rollback_path} && \
         {sudo}mv -f {staged_path} {AGENT_PATH} && \
         {sudo}rm -f {remote_upload} && \
         {sudo}systemctl daemon-reload && {sudo}systemctl restart hope-agent",
    );
    let started_at = time::OffsetDateTime::now_utc();
    let restart_result = run_command(&mut session, &install_and_restart, config.timeout).await;
    if restart_result.is_err()
        || !wait_for_agent_check_in(
            pool,
            request.agent_id,
            &request.target_version,
            started_at,
            bounded_deadline(request.check_in_deadline),
        )
        .await?
    {
        let rollback_result = rollback_remote(
            &mut session,
            &sudo,
            &remote_upload,
            &staged_path,
            &rollback_path,
            config.timeout,
        )
        .await;
        if rollback_result.is_ok()
            && wait_for_previous_agent(
                pool,
                request.agent_id,
                request.previous_version.as_deref(),
                time::OffsetDateTime::now_utc(),
                bounded_deadline(request.check_in_deadline.min(Duration::from_secs(60))),
            )
            .await?
        {
            let _ = session
                .disconnect(russh::Disconnect::ByApplication, "rolled back", "")
                .await;
            drop(secret);
            return Ok(AgentUpdateResult {
                state: AgentUpdateState::RolledBack,
                platform: platform.to_string(),
                architecture: architecture.to_string(),
            });
        }

        let _ = session
            .disconnect(russh::Disconnect::ByApplication, "update failed", "")
            .await;
        drop(secret);
        if request.repair_on_failure {
            install_or_repair(
                pool,
                store,
                DeploymentRequest {
                    device_id: request.device_id,
                    host,
                    port: request.port,
                    credential_id: request.credential_id,
                    actor_user_id: request.actor_user_id,
                    repair: true,
                    disassociate_after_enrollment: false,
                    connection_url,
                },
            )
            .await?;
            return Ok(AgentUpdateResult {
                state: AgentUpdateState::Repaired,
                platform: platform.to_string(),
                architecture: architecture.to_string(),
            });
        }
        return Err(InstallError::ServiceUnavailable);
    }

    let _ = run_command(
        &mut session,
        &format!("{sudo}rm -f {rollback_path} {staged_path} {remote_upload}"),
        config.timeout,
    )
    .await;
    let _ = session
        .disconnect(russh::Disconnect::ByApplication, "updated", "")
        .await;
    drop(secret);
    Ok(AgentUpdateResult {
        state: AgentUpdateState::Succeeded,
        platform: platform.to_string(),
        architecture: architecture.to_string(),
    })
}

async fn rollback_remote(
    session: &mut client::Handle<HostKeyHandler>,
    sudo: &str,
    remote_upload: &str,
    staged_path: &str,
    rollback_path: &str,
    timeout: Duration,
) -> Result<CommandOutput> {
    let command = format!(
        "{sudo}sh -c {}",
        sh_quote(&format!(
            "test -f {rollback_path} && mv -f {rollback_path} {AGENT_PATH} && rm -f {remote_upload} {staged_path} && systemctl daemon-reload && systemctl restart hope-agent"
        )),
    );
    run_command(session, &command, timeout).await
}

async fn wait_for_first_check_in(
    pool: &PgPool,
    agent_id: Uuid,
    started_at: time::OffsetDateTime,
    deadline: Duration,
) -> Result<bool> {
    let until = tokio::time::Instant::now() + deadline;
    loop {
        let ready: bool = sqlx::query_scalar(
            "select exists(select 1 from agents where id = $1 and revoked_at is null and last_seen >= $2)",
        )
        .bind(agent_id)
        .bind(started_at)
        .fetch_one(pool)
        .await?;
        if ready {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= until {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn wait_for_agent_check_in(
    pool: &PgPool,
    agent_id: Uuid,
    version: &str,
    started_at: time::OffsetDateTime,
    deadline: Duration,
) -> Result<bool> {
    let deadline_at = tokio::time::Instant::now() + deadline;
    loop {
        let ready: bool = sqlx::query_scalar(
            "select exists(\
                 select 1 from agents where id = $1 and revoked_at is null\
                   and agent_version = $2 and last_seen >= $3)",
        )
        .bind(agent_id)
        .bind(version)
        .bind(started_at)
        .fetch_one(pool)
        .await?;
        if ready {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline_at {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn wait_for_previous_agent(
    pool: &PgPool,
    agent_id: Uuid,
    version: Option<&str>,
    started_at: time::OffsetDateTime,
    deadline: Duration,
) -> Result<bool> {
    let Some(version) = version else {
        return Ok(true);
    };
    wait_for_agent_check_in(pool, agent_id, version, started_at, deadline).await
}

fn bounded_deadline(deadline: Duration) -> Duration {
    if deadline.is_zero() {
        DEFAULT_UPDATE_DEADLINE
    } else {
        deadline.clamp(Duration::from_secs(10), MAX_UPDATE_TIMEOUT)
    }
}

fn validate_update_version(version: &str) -> Result<()> {
    if version.is_empty()
        || version.len() > 128
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(InstallError::Configuration);
    }
    Ok(())
}

async fn connect_authenticated(
    pool: &PgPool,
    device_id: Option<Uuid>,
    host: &str,
    port: i32,
    actor_user_id: Option<Uuid>,
    secret: &CredentialSecret,
    timeout: Duration,
) -> Result<(
    client::Handle<HostKeyHandler>,
    Arc<Mutex<Option<HostKeyDecision>>>,
)> {
    let decision = Arc::new(Mutex::new(None));
    let handler = HostKeyHandler {
        pool: pool.clone(),
        device_id,
        host: host.to_string(),
        port,
        actor_user_id,
        decision: decision.clone(),
    };
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(timeout),
        ..Default::default()
    });
    let address = format!("{host}:{port}");
    let mut session =
        match tokio::time::timeout(timeout, client::connect(config, address, handler)).await {
            Err(_) => return Err(InstallError::Timeout),
            Ok(Err(error)) => {
                let decision_value = decision.lock().await.clone();
                let permits = decision_value
                    .as_ref()
                    .is_some_and(HostKeyDecision::permits_connection);
                tracing::warn!(
                    host = %host,
                    port,
                    permits,
                    decision = ?decision_value,
                    error = %error,
                    "SSH connection failed before authentication"
                );
                if !permits {
                    return Err(InstallError::HostKeyUntrusted);
                }
                return Err(InstallError::Connection);
            }
            Ok(Ok(session)) => session,
        };
    let auth = match secret {
        CredentialSecret::SshPassword { username, password } => tokio::time::timeout(
            timeout,
            session.authenticate_password(username.as_str(), password.as_str()),
        )
        .await
        .map_err(|_| InstallError::Timeout)?
        .map_err(|_| InstallError::Authentication)?,
        CredentialSecret::SshPrivateKey {
            username,
            private_key_pem,
            passphrase,
        } => {
            let private_key = russh::keys::decode_secret_key(
                private_key_pem.as_str(),
                passphrase.as_ref().map(|value| value.as_str()),
            )
            .map_err(|_| InstallError::Authentication)?;
            let private_key = PrivateKeyWithHashAlg::new(Arc::new(private_key), None);
            tokio::time::timeout(
                timeout,
                session.authenticate_publickey(username.as_str(), private_key),
            )
            .await
            .map_err(|_| InstallError::Timeout)?
            .map_err(|_| InstallError::Authentication)?
        }
    };
    if !matches!(auth, AuthResult::Success) {
        return Err(InstallError::Authentication);
    }
    Ok((session, decision))
}

async fn run_command(
    session: &mut client::Handle<HostKeyHandler>,
    command: &str,
    timeout: Duration,
) -> Result<CommandOutput> {
    let mut channel = tokio::time::timeout(timeout, session.channel_open_session())
        .await
        .map_err(|_| InstallError::Timeout)?
        .map_err(|_| InstallError::Connection)?;
    tokio::time::timeout(timeout, channel.exec(true, command.as_bytes()))
        .await
        .map_err(|_| InstallError::Timeout)?
        .map_err(|_| InstallError::Command)?;
    collect_channel(&mut channel, timeout).await
}

async fn run_with_stdin(
    session: &mut client::Handle<HostKeyHandler>,
    command: &str,
    input: &[u8],
    timeout: Duration,
) -> Result<CommandOutput> {
    let mut channel = tokio::time::timeout(timeout, session.channel_open_session())
        .await
        .map_err(|_| InstallError::Timeout)?
        .map_err(|_| InstallError::Connection)?;
    tokio::time::timeout(timeout, channel.exec(true, command.as_bytes()))
        .await
        .map_err(|_| InstallError::Timeout)?
        .map_err(|_| InstallError::Command)?;
    tokio::time::timeout(timeout, channel.data_bytes(input.to_vec()))
        .await
        .map_err(|_| InstallError::Timeout)?
        .map_err(|_| InstallError::Command)?;
    tokio::time::timeout(timeout, channel.eof())
        .await
        .map_err(|_| InstallError::Timeout)?
        .map_err(|_| InstallError::Command)?;
    let mut output = collect_channel(&mut channel, timeout).await?;
    if input.len() <= 8 * 1024
        && input
            .iter()
            .all(|byte| byte.is_ascii_graphic() || byte.is_ascii_whitespace())
    {
        let input_text = String::from_utf8_lossy(input);
        let mut sensitive_values = vec![input_text.as_ref()];
        if let Some((token, fingerprint)) = input_text.trim().split_once('.') {
            sensitive_values.push(token);
            sensitive_values.push(fingerprint);
        }
        output.stdout = redact_sensitive(&output.stdout, &sensitive_values);
        output.stderr = redact_sensitive(&output.stderr, &sensitive_values);
    }
    Ok(output)
}

async fn collect_channel(
    channel: &mut russh::Channel<russh::client::Msg>,
    timeout: Duration,
) -> Result<CommandOutput> {
    let result = tokio::time::timeout(timeout, async {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_status = None;
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { data } => append_bounded(&mut stdout, &data)?,
                ChannelMsg::ExtendedData { data, .. } => append_bounded(&mut stderr, &data)?,
                ChannelMsg::ExitStatus {
                    exit_status: status,
                } => {
                    exit_status = Some(status);
                    break;
                }
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok::<_, InstallError>((stdout, stderr, exit_status))
    })
    .await
    .map_err(|_| InstallError::Timeout)??;
    let _ = channel.close().await;
    let (stdout, stderr, exit_status) = result;
    if exit_status.is_some_and(|status| status != 0) {
        return Err(InstallError::Command);
    }
    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        exit_status,
    })
}

fn append_bounded(buffer: &mut Vec<u8>, data: &[u8]) -> Result<()> {
    if buffer.len().saturating_add(data.len()) > MAX_COMMAND_OUTPUT {
        return Err(InstallError::OutputLimit);
    }
    buffer.extend_from_slice(data);
    Ok(())
}

#[derive(Debug)]
struct CommandOutput {
    stdout: String,
    #[allow(dead_code)]
    stderr: String,
    #[allow(dead_code)]
    exit_status: Option<u32>,
}

async fn privilege_prefix(
    session: &mut client::Handle<HostKeyHandler>,
    timeout: Duration,
) -> Result<String> {
    let uid = run_command(session, "id -u", timeout).await?;
    if uid.stdout.trim() == "0" {
        return Ok(String::new());
    }
    run_command(session, "sudo -n true", timeout)
        .await
        .map_err(|_| InstallError::Authentication)?;
    Ok("sudo -n ".to_string())
}

async fn upload_binary(
    session: &mut client::Handle<HostKeyHandler>,
    sudo: &str,
    remote_path: &str,
    binary: &[u8],
    timeout: Duration,
) -> Result<()> {
    let command = format!("{sudo}sh -c {}", sh_quote(&format!("cat > {remote_path}")));
    run_with_stdin(session, &command, binary, timeout).await?;
    Ok(())
}

async fn prepare_service_account(
    session: &mut client::Handle<HostKeyHandler>,
    sudo: &str,
    config: &DeploymentConfig,
    timeout: Duration,
) -> Result<()> {
    let inner = format!(
        "if ! id -u {user} >/dev/null 2>&1; then useradd --system --home-dir {state} --shell /usr/sbin/nologin {user}; fi; install -d -o {user} -g {user} -m 0700 {state}",
        user = sh_quote(&config.service_user),
        state = sh_quote(&config.state_dir),
    );
    run_command(
        session,
        &format!("{sudo}sh -c {}", sh_quote(&inner)),
        timeout,
    )
    .await?;
    Ok(())
}

async fn install_service_unit(
    session: &mut client::Handle<HostKeyHandler>,
    sudo: &str,
    config: &DeploymentConfig,
    timeout: Duration,
) -> Result<()> {
    let unit = format!(
        "[Unit]\nDescription=Hope agent\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nUser={}\nGroup={}\nExecStart={} run --gateway {} --state-dir {}\nRestart=always\nRestartSec=5\nNoNewPrivileges=true\nPrivateTmp=true\nProtectHome=true\nProtectSystem=strict\nReadWritePaths={}\n\n[Install]\nWantedBy=multi-user.target\n",
        config.service_user,
        config.service_user,
        AGENT_PATH,
        config.gateway_url,
        config.state_dir,
        config.state_dir,
    );
    let command = format!("{sudo}sh -c {}", sh_quote(&format!("cat > {SERVICE_PATH}")));
    run_with_stdin(session, &command, unit.as_bytes(), timeout).await?;
    Ok(())
}

fn tls_fingerprint_from_environment() -> Result<String> {
    let path = std::env::var("HOPE_SERVER_CERT_PATH")
        .unwrap_or_else(|_| "data/pki/server-cert.pem".into());
    let pem = std::fs::read(path).map_err(InstallError::LocalFile)?;
    let mut pem = pem.as_slice();
    let cert = rustls_pemfile::certs(&mut pem)
        .next()
        .ok_or(InstallError::Configuration)?
        .map_err(|_| InstallError::Configuration)?;
    let mut hasher = Sha256::new();
    hasher.update(cert.as_ref());
    Ok(hex::encode(hasher.finalize()))
}

fn connection_endpoints(connection_url: &str) -> Result<(String, String, bool)> {
    let mut url = reqwest::Url::parse(connection_url).map_err(|_| InstallError::Configuration)?;
    let system_tls = match url.scheme() {
        "http" => false,
        "https" => true,
        _ => return Err(InstallError::Configuration),
    };
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || connection_url.len() > 1024
    {
        return Err(InstallError::Configuration);
    }
    url.set_scheme("https")
        .map_err(|_| InstallError::Configuration)?;
    let enroll_url = url.as_str().trim_end_matches('/').to_string();
    url.set_scheme("wss")
        .map_err(|_| InstallError::Configuration)?;
    url.set_path("/agent/v1/connect");
    Ok((enroll_url, url.to_string(), system_tls))
}

fn validate_url(value: &str, scheme: &str) -> Result<()> {
    if !value.starts_with(scheme)
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(InstallError::Configuration);
    }
    Ok(())
}

fn validate_state_dir(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 256
        || !value.starts_with('/')
        || value == "/"
        || value.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '/' | '-' | '_' | '.'))
        })
        || value
            .split('/')
            .any(|component| matches!(component, "." | ".."))
    {
        return Err(InstallError::Configuration);
    }
    Ok(())
}

fn validate_service_user(value: &str) -> Result<()> {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return Err(InstallError::Configuration);
    };
    if value.len() > 64
        || !(first.is_ascii_alphanumeric() || first == '_')
        || !characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
    {
        return Err(InstallError::Configuration);
    }
    Ok(())
}

fn validate_unit_argument(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 1024
        || value.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '\\' | '\'' | '"')
        })
    {
        return Err(InstallError::Configuration);
    }
    Ok(())
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn single_line(value: &str) -> Option<&str> {
    value.lines().map(str::trim).find(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_never_interprets_single_quotes() {
        assert_eq!(sh_quote("safe value"), "'safe value'");
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn binary_output_is_bounded() {
        let mut output = Vec::new();
        assert!(append_bounded(&mut output, &[0; MAX_COMMAND_OUTPUT]).is_ok());
        assert!(matches!(
            append_bounded(&mut output, &[1]),
            Err(InstallError::OutputLimit)
        ));
    }

    #[test]
    fn only_supported_platforms_are_selected() {
        assert_eq!(single_line("Linux\n"), Some("Linux"));
        assert_eq!(single_line("\n aarch64\n"), Some("aarch64"));
        assert_eq!(single_line(""), None);
    }

    #[test]
    fn remote_paths_and_unit_values_are_strictly_bounded() {
        assert!(validate_state_dir("/var/lib/hope").is_ok());
        assert!(validate_state_dir("/").is_err());
        assert!(validate_state_dir("relative/path").is_err());
        assert!(validate_state_dir("/var/lib/../tmp").is_err());
        assert!(validate_service_user("hope-agent").is_ok());
        assert!(validate_service_user("hope agent").is_err());
        assert!(validate_unit_argument("wss://hope.example:8443").is_ok());
        assert!(validate_unit_argument("wss://host/'bad'").is_err());
    }

    #[test]
    fn connection_address_selects_pinned_lan_or_system_proxy_tls() {
        assert_eq!(
            connection_endpoints("http://192.168.1.163").unwrap(),
            (
                "https://192.168.1.163".to_string(),
                "wss://192.168.1.163/agent/v1/connect".to_string(),
                false,
            )
        );
        assert_eq!(
            connection_endpoints("https://hope.example").unwrap(),
            (
                "https://hope.example".to_string(),
                "wss://hope.example/agent/v1/connect".to_string(),
                true,
            )
        );
        assert!(connection_endpoints("https://user:pass@hope.example").is_err());
        assert!(connection_endpoints("https://hope.example/path").is_err());
    }
}
