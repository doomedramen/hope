use crate::{
    agents,
    error::{ApiError, ApiResult},
    state::{State, hash, now},
};
use axum::{Json, extract::State as Extract};
use hope_protocol::InstallJob;
use russh::{
    ChannelMsg, client,
    keys::{PrivateKeyWithHashAlg, ssh_key},
};
use serde::Deserialize;
use serde_json::json;
use std::{net::IpAddr, sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::timeout};
use uuid::Uuid;

struct PinnedClient {
    expected: Option<String>,
    observed: Arc<Mutex<Option<String>>>,
}
impl client::Handler for PinnedClient {
    type Error = russh::Error;
    async fn check_server_key(&mut self, key: &ssh_key::PublicKey) -> Result<bool, Self::Error> {
        let fingerprint = key.fingerprint(ssh_key::HashAlg::Sha256).to_string();
        *self.observed.lock().await = Some(fingerprint.clone());
        // A fingerprint-only connection never authenticates or executes commands.
        Ok(self
            .expected
            .as_ref()
            .is_none_or(|expected| *expected == fingerprint))
    }
}
#[derive(Deserialize)]
pub struct Target {
    pub host_id: String,
    pub address: String,
    pub port: Option<u16>,
}
async fn validate_target(state: &State, target: &Target) -> ApiResult<IpAddr> {
    let host = state
        .host(&target.host_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !host.addresses.contains(&target.address) {
        return Err(ApiError::bad("Choose an address belonging to this host"));
    }
    target
        .address
        .parse()
        .map_err(|_| ApiError::bad("SSH requires an IP address"))
}
fn config() -> Arc<client::Config> {
    Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(30)),
        ..Default::default()
    })
}
pub async fn fingerprint(
    Extract(state): Extract<State>,
    Json(target): Json<Target>,
) -> ApiResult<Json<serde_json::Value>> {
    let ip = validate_target(&state, &target).await?;
    let observed = Arc::new(Mutex::new(None));
    let session = timeout(
        Duration::from_secs(10),
        client::connect(
            config(),
            (ip, target.port.unwrap_or(22)),
            PinnedClient {
                expected: None,
                observed: observed.clone(),
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
    Ok(Json(
        json!({"fingerprint":fingerprint,"instruction":"Verify this fingerprint through a trusted channel before installation."}),
    ))
}
#[derive(Deserialize)]
pub struct InstallRequest {
    #[serde(flatten)]
    pub target: Target,
    pub username: String,
    pub fingerprint: String,
    pub password: Option<String>,
    pub private_key: Option<String>,
    pub passphrase: Option<String>,
}
pub async fn install(
    Extract(state): Extract<State>,
    Json(request): Json<InstallRequest>,
) -> ApiResult<Json<InstallJob>> {
    let ip = validate_target(&state, &request.target).await?;
    if request.username.is_empty()
        || request.username.len() > 128
        || !request.fingerprint.starts_with("SHA256:")
        || request.fingerprint.len() > 100
    {
        return Err(ApiError::bad(
            "Username and verified SHA256 fingerprint are required",
        ));
    }
    if request.password.is_none() && request.private_key.is_none() {
        return Err(ApiError::bad("Provide a password or private key"));
    }
    let mut jobs = state.0.installs.lock().await;
    jobs.retain(|_, j| j.created_at > now() - 86400.0);
    if jobs.values().filter(|j| j.status == "running").count() >= 4 {
        return Err(ApiError::bad(
            "At most four SSH installations may run concurrently",
        ));
    }
    if jobs.len() >= 100 {
        return Err(ApiError::bad(
            "Installation history limit reached; retry after older jobs expire",
        ));
    }
    let job = InstallJob {
        id: Uuid::new_v4().to_string(),
        host: request.target.address.clone(),
        status: "running".into(),
        messages: vec!["Connecting with the verified host key".into()],
        created_at: now(),
    };
    jobs.insert(job.id.clone(), job.clone());
    drop(jobs);
    let task_state = state.clone();
    let id = job.id.clone();
    tokio::spawn(async move {
        let result = timeout(
            Duration::from_secs(300),
            provision(&task_state, &id, ip, request),
        )
        .await;
        let (status, message) = match result {
            Ok(Ok(())) => (
                "completed",
                "Agent enrolled and first report received".to_string(),
            ),
            Ok(Err(e)) => ("failed", e.to_string()),
            Err(_) => (
                "failed",
                "Installation exceeded five minutes; inspect the host before retrying".into(),
            ),
        };
        if let Some(job) = task_state.0.installs.lock().await.get_mut(&id) {
            job.status = status.into();
            job.messages.push(message);
        }
        task_state.changed();
    });
    Ok(Json(job))
}
async fn progress(state: &State, id: &str, message: &str) {
    if let Some(job) = state.0.installs.lock().await.get_mut(id) {
        job.messages.push(message.into());
    }
    state.changed();
}
async fn provision(
    state: &State,
    id: &str,
    ip: IpAddr,
    request: InstallRequest,
) -> anyhow::Result<()> {
    let mut session = timeout(
        Duration::from_secs(15),
        client::connect(
            config(),
            (ip, request.target.port.unwrap_or(22)),
            PinnedClient {
                expected: Some(request.fingerprint),
                observed: Arc::new(Mutex::new(None)),
            },
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("SSH connection timed out"))?
    .map_err(|_| anyhow::anyhow!("SSH connection failed or host key changed"))?;
    let authenticated = if let Some(private) = request.private_key.filter(|s| !s.is_empty()) {
        let key = russh::keys::decode_secret_key(&private, request.passphrase.as_deref())
            .map_err(|_| anyhow::anyhow!("Could not decrypt or parse the SSH private key"))?;
        let algorithm = session.best_supported_rsa_hash().await?.flatten();
        session
            .authenticate_publickey(
                &request.username,
                PrivateKeyWithHashAlg::new(Arc::new(key), algorithm),
            )
            .await?
            .success()
    } else {
        session
            .authenticate_password(&request.username, request.password.unwrap_or_default())
            .await?
            .success()
    };
    anyhow::ensure!(authenticated, "SSH authentication failed");
    progress(
        state,
        id,
        "Authenticated. Running installer; non-interactive sudo or a user service will be used.",
    )
    .await;
    let token = agents::issue_token(state)
        .await
        .map_err(|_| anyhow::anyhow!("Could not create enrollment token"))?;
    let command = agents::install_command(state, &token);
    let mut channel = session.channel_open_session().await?;
    channel.exec(true, "sh -s").await?;
    channel.data(format!("{command}\n").as_bytes()).await?;
    channel.eof().await?;
    let mut code = None;
    // Remote output is deliberately not retained: arbitrary stdout can contain credentials.
    while let Some(message) = channel.wait().await {
        if let ChannelMsg::ExitStatus { exit_status } = message {
            code = Some(exit_status);
        }
    }
    let _ = session
        .disconnect(
            russh::Disconnect::ByApplication,
            "Installation finished",
            "en",
        )
        .await;
    anyhow::ensure!(
        code == Some(0),
        "Installer failed. Check curl/OpenSSL, CA trust, service-manager support, and non-interactive sudo on the host."
    );
    let agent: Option<String> =
        sqlx::query_scalar("SELECT agent_id FROM enrollment_tokens WHERE token_hash=?")
            .bind(hash(&token))
            .fetch_optional(&state.0.db)
            .await?
            .flatten();
    let Some(agent) = agent else {
        anyhow::bail!(
            "Installer exited without enrolling an agent; an existing installation may already be enrolled"
        );
    };
    let hosts = state.hosts().await?;
    anyhow::ensure!(
        hosts.iter().any(|h| h.agent_id.as_ref() == Some(&agent)
            && h.last_report.is_some_and(|t| t > now() - 300.0)),
        "Enrollment succeeded but no report arrived"
    );
    Ok(())
}
pub async fn jobs(Extract(state): Extract<State>) -> Json<Vec<InstallJob>> {
    let mut jobs: Vec<_> = state.0.installs.lock().await.values().cloned().collect();
    jobs.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
    Json(jobs)
}
