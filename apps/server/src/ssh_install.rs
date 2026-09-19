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
use crate::ssh_trust::{self, HostKeyDecision};

const MAX_COMMAND_OUTPUT: usize = 64 * 1024;
const MAX_AGENT_BINARY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
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
struct DeploymentConfig {
    enroll_url: String,
    gateway_url: String,
    binary_x86_64: String,
    binary_aarch64: String,
    state_dir: String,
    service_user: String,
    timeout: Duration,
}

impl DeploymentConfig {
    fn from_environment() -> Result<Self> {
        let enroll_url = required_env("HOPE_AGENT_ENROLL_URL")?;
        let gateway_url = required_env("HOPE_AGENT_GATEWAY_URL")?;
        let binary_x86_64 = required_env("HOPE_AGENT_BINARY_X86_64")?;
        let binary_aarch64 = required_env("HOPE_AGENT_BINARY_AARCH64")?;
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
            binary_x86_64,
            binary_aarch64,
            state_dir,
            service_user,
            timeout: Duration::from_secs(timeout_seconds),
        })
    }

    fn binary_path(&self, architecture: &str) -> Result<&str> {
        match architecture {
            "x86_64" => Ok(&self.binary_x86_64),
            "aarch64" => Ok(&self.binary_aarch64),
            _ => Err(InstallError::UnsupportedHost),
        }
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
        .await?;
        let permits = decision.permits_connection();
        *self.decision.lock().await = Some(decision);
        Ok(permits)
    }
}

pub async fn install_or_repair(
    pool: &PgPool,
    store: &CredentialStore,
    request: DeploymentRequest,
) -> Result<DeploymentResult> {
    let config = DeploymentConfig::from_environment()?;
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
    if !matches!(architecture, "x86_64" | "aarch64") {
        return Err(InstallError::UnsupportedHost);
    }

    let binary_path = config.binary_path(architecture)?;
    let binary = read_binary(binary_path).await?;
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
            "{sudo}test -s {state}/agent-key.pem && test -s {state}/agent-cert.pem && \
             test -s {state}/ca-cert.pem && echo enrolled || echo missing",
            state = sh_quote(&config.state_dir),
        ),
        config.timeout,
    )
    .await?
    .stdout;
    let mut enrolled = false;
    if state_present.trim() != "enrolled" {
        let token = agents::create_enrollment_token(pool, 15).await?;
        let ca_fingerprint = ca_fingerprint_from_environment()?;
        let code = format!("{token}.{ca_fingerprint}");
        let user_prefix = if sudo.is_empty() {
            format!("runuser -u {} -- ", sh_quote(&config.service_user))
        } else {
            format!("{sudo}-u {} ", sh_quote(&config.service_user))
        };
        let command = format!(
            "{user_prefix}{AGENT_PATH} enroll --server {} --code-stdin --state-dir {}",
            sh_quote(&config.enroll_url),
            sh_quote(&config.state_dir),
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

    if request.disassociate_after_enrollment && enrolled {
        store
            .disassociate_device(
                request.credential_id,
                request.device_id,
                request.actor_user_id,
            )
            .await?;
    }

    install_service_unit(&mut session, &sudo, &config, config.timeout).await?;
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
            Ok(Err(_)) => {
                if !decision
                    .lock()
                    .await
                    .as_ref()
                    .is_some_and(HostKeyDecision::permits_connection)
                {
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

async fn read_binary(path: &str) -> Result<Vec<u8>> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(InstallError::LocalFile)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_AGENT_BINARY_BYTES {
        return Err(InstallError::BinaryUnavailable);
    }
    tokio::fs::read(path).await.map_err(InstallError::LocalFile)
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

fn ca_fingerprint_from_environment() -> Result<String> {
    let path = std::env::var("HOPE_CA_CERT_PATH").unwrap_or_else(|_| "data/pki/ca-cert.pem".into());
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

fn required_env(name: &'static str) -> Result<String> {
    let value = std::env::var(name).map_err(|_| InstallError::Configuration)?;
    if value.trim().is_empty() || value.len() > 1024 {
        return Err(InstallError::Configuration);
    }
    Ok(value)
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
}
