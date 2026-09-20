//! M7 release compliance, update policy, and SSH update orchestration.
//!
//! The API never accepts an arbitrary binary or shell command. It selects a
//! verified artifact from the configured release repository, persists a
//! secret-free job, and lets the worker use the already-scoped M6 SSH vault.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use jobs::enqueue_in;
use semver::Version;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::credentials::CredentialStore;
use crate::inventory::events::Recorder;
use crate::jobs_handlers::JobOutcome;
use crate::release_repository::{ReleaseRepository, RepositoryError, VerifiedRelease};
use crate::ssh_install;
use crate::ssh_trust;
use crate::state::AppState;

pub const UPDATE_JOB_TYPE: &str = "agent.update";
pub const RECONCILE_JOB_TYPE: &str = "agent_updates.reconcile";

const DEFAULT_DEADLINE_SECONDS: i64 = 120;
const MAX_DEADLINE_SECONDS: i64 = 30 * 60;

#[derive(Debug, Deserialize)]
pub struct StartUpdateRequest {
    pub device_id: Uuid,
    pub host: String,
    pub port: i32,
    pub credential_id: Uuid,
    pub version: String,
    pub deadline_seconds: Option<i64>,
    #[serde(default = "default_true")]
    pub repair_on_failure: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePolicyRequest {
    pub mode: String,
    #[serde(default = "default_channel")]
    pub channel: String,
    pub pinned_version: Option<String>,
    #[serde(default = "default_rollout")]
    pub rollout_percent: i32,
    pub device_id: Uuid,
    pub host: String,
    pub port: i32,
    pub credential_id: Uuid,
    #[serde(default = "default_true")]
    pub repair_on_failure: bool,
}

fn default_true() -> bool {
    true
}

fn default_channel() -> String {
    "stable".to_string()
}

fn default_rollout() -> i32 {
    100
}

fn error(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.into() })))
}

fn idempotency_key(headers: &HeaderMap) -> Result<&str, (StatusCode, Json<Value>)> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 255)
        .ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "Idempotency-Key header is required",
            )
        })
}

fn deadline_seconds(value: Option<i64>) -> Result<i64, (StatusCode, Json<Value>)> {
    let value = value.unwrap_or(DEFAULT_DEADLINE_SECONDS);
    if !(10..=MAX_DEADLINE_SECONDS).contains(&value) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!("deadline_seconds must be between 10 and {MAX_DEADLINE_SECONDS}"),
        ));
    }
    Ok(value)
}

fn platform_arch(os: &str, arch: &str) -> Option<(&'static str, &'static str)> {
    if os != "linux" {
        return None;
    }
    let arch = match arch {
        "x86_64" | "amd64" => "amd64",
        "aarch64" | "arm64" => "arm64",
        _ => return None,
    };
    Some(("linux", arch))
}

fn repository_error(error: &RepositoryError) -> (StatusCode, String) {
    match error {
        RepositoryError::NotFound => (StatusCode::NOT_FOUND, "release not found".to_string()),
        RepositoryError::Incompatible => (
            StatusCode::CONFLICT,
            "release is incompatible with this agent".to_string(),
        ),
        RepositoryError::InvalidVersion => (
            StatusCode::BAD_REQUEST,
            "release version is invalid".to_string(),
        ),
        RepositoryError::NoTrustedKeys
        | RepositoryError::TooManyTrustedKeys { .. }
        | RepositoryError::InvalidPublicKey
        | RepositoryError::InvalidManifestSignature
        | RepositoryError::InvalidArtifactSignature
        | RepositoryError::ArtifactVerification(_)
        | RepositoryError::Manifest(_)
        | RepositoryError::ManifestTooLarge
        | RepositoryError::SignatureTooLarge
        | RepositoryError::ArtifactMissing
        | RepositoryError::ArtifactNotRegular
        | RepositoryError::ArtifactTooLarge
        | RepositoryError::Unavailable
        | RepositoryError::NotConfigured
        | RepositoryError::File(_)
        | RepositoryError::Json(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "verified release repository is unavailable".to_string(),
        ),
    }
}

async fn verified_release(
    pool: &PgPool,
    version: &str,
) -> Result<(ReleaseRepository, VerifiedRelease), (StatusCode, Json<Value>)> {
    let repository = match ReleaseRepository::from_environment() {
        Ok(repository) => repository,
        Err(repo_error) => {
            let (status, message) = repository_error(&repo_error);
            return Err(error(status, message));
        }
    };
    let release = match repository.load(version) {
        Ok(release) => release,
        Err(repo_error) => {
            let (status, message) = repository_error(&repo_error);
            return Err(error(status, message));
        }
    };
    record_release(pool, &release).await.map_err(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not record release metadata",
        )
    })?;
    Ok((repository, release))
}

async fn record_release(pool: &PgPool, release: &VerifiedRelease) -> sqlx::Result<()> {
    sqlx::query(
        "insert into agent_release_versions \
             (version, manifest, manifest_sha256, signing_key_fingerprint) \
         values ($1, $2, $3, $4) \
         on conflict (version) do update set manifest = excluded.manifest, \
             manifest_sha256 = excluded.manifest_sha256, \
             signing_key_fingerprint = excluded.signing_key_fingerprint, \
             last_seen_at = now(), updated_at = now()",
    )
    .bind(&release.manifest.version)
    .bind(
        serde_json::to_value(&release.manifest)
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
    )
    .bind(&release.manifest_sha256)
    .bind(&release.manifest_signing_key_fingerprint)
    .execute(pool)
    .await?;
    Ok(())
}

/// GET /api/v1/agent-releases
pub async fn list_releases(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let repository = ReleaseRepository::from_environment().map_err(|repo_error| {
        let (status, message) = repository_error(&repo_error);
        error(status, message)
    })?;
    let releases = repository.list().map_err(|repo_error| {
        let (status, message) = repository_error(&repo_error);
        error(status, message)
    })?;
    let mut items = Vec::with_capacity(releases.len());
    for release in releases {
        record_release(&state.pool, &release).await.map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not record release metadata",
            )
        })?;
        items.push(release.summary());
    }
    Ok(Json(json!({ "items": items })))
}

/// GET /api/v1/agent-releases/:version
pub async fn get_release(
    State(state): State<AppState>,
    Path(version): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, release) = verified_release(&state.pool, &version).await?;
    Ok(Json(json!(release.summary())))
}

async fn target_metadata(
    pool: &PgPool,
    agent_id: Uuid,
) -> Result<(String, String, u32, String), (StatusCode, Json<Value>)> {
    let row = sqlx::query(
        "select agent_version, os, arch, protocol_version, revoked_at \
         from agents where id = $1",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "could not read agent"))?;
    let Some(row) = row else {
        return Err(error(StatusCode::NOT_FOUND, "agent not found"));
    };
    if row
        .try_get::<Option<time::OffsetDateTime>, _>("revoked_at")
        .ok()
        .flatten()
        .is_some()
    {
        return Err(error(StatusCode::CONFLICT, "agent is revoked"));
    }
    Ok((
        row.try_get("agent_version").unwrap_or_default(),
        row.try_get("os").unwrap_or_default(),
        u32::try_from(row.try_get::<i32, _>("protocol_version").unwrap_or(0)).unwrap_or(0),
        row.try_get("arch").unwrap_or_default(),
    ))
}

async fn validate_device_and_credential(
    transaction: &mut Transaction<'_, Postgres>,
    device_id: Uuid,
    credential_id: Uuid,
) -> Result<(), (StatusCode, Json<Value>)> {
    let device_exists: Option<(Uuid,)> =
        sqlx::query_as("select id from devices where id = $1 for key share")
            .bind(device_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "could not verify device"))?;
    if device_exists.is_none() {
        return Err(error(StatusCode::NOT_FOUND, "device not found"));
    }
    let association: Option<(Uuid,)> = sqlx::query_as(
        "insert into credential_associations (credential_id, device_id, purpose) \
         select $1, $2, 'agent_install' from credentials \
         where id = $1 and deleted_at is null and revoked_at is null \
           and kind in ('ssh_private_key', 'ssh_password') \
         on conflict (credential_id, device_id, purpose) do update \
             set disassociated_at = null returning id",
    )
    .bind(credential_id)
    .bind(device_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not associate credential",
        )
    })?;
    if association.is_none() {
        return Err(error(StatusCode::NOT_FOUND, "credential not found"));
    }
    Ok(())
}

struct EnqueueUpdate<'a> {
    request: &'a StartUpdateRequest,
    actor_user_id: Option<Uuid>,
    agent_id: Uuid,
    previous_version: &'a str,
    platform: &'a str,
    architecture: &'a str,
    deadline: i64,
}

async fn enqueue_update_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    idempotency: &str,
    context: EnqueueUpdate<'_>,
) -> anyhow::Result<(Uuid, Uuid)> {
    let EnqueueUpdate {
        request,
        actor_user_id,
        agent_id,
        previous_version,
        platform,
        architecture,
        deadline,
    } = context;
    let payload = json!({
        "agent_id": agent_id,
        "device_id": request.device_id,
        "host": request.host,
        "port": request.port,
        "credential_id": request.credential_id,
        "actor_user_id": actor_user_id,
        "target_version": request.version,
        "previous_version": if previous_version.is_empty() { Value::Null } else { json!(previous_version) },
        "platform": platform,
        "architecture": architecture,
        "deadline_seconds": deadline,
        "repair_on_failure": request.repair_on_failure,
    });
    let job_id = enqueue_in(transaction, UPDATE_JOB_TYPE, idempotency, payload).await?;
    if let Some((operation_id,)) =
        sqlx::query_as::<_, (Uuid,)>("select id from agent_update_operations where job_id = $1")
            .bind(job_id)
            .fetch_optional(&mut **transaction)
            .await?
    {
        return Ok((job_id, operation_id));
    }
    let operation_id: Uuid = sqlx::query_scalar(
        "insert into agent_update_operations \
             (agent_id, job_id, device_id, target_version, previous_version, deadline_at, created_by) \
         values ($1, $2, $3, $4, nullif($5, ''), now() + make_interval(secs => $6), $7) \
         returning id",
    )
    .bind(agent_id)
    .bind(job_id)
    .bind(request.device_id)
    .bind(&request.version)
    .bind(previous_version)
    .bind(deadline as f64)
    .bind(actor_user_id)
    .fetch_one(&mut **transaction)
    .await?;
    Ok((job_id, operation_id))
}

/// POST /api/v1/agents/:id/update
pub async fn start_update(
    State(state): State<AppState>,
    Extension(CurrentUser(actor_user_id)): Extension<CurrentUser>,
    Path(agent_id): Path<Uuid>,
    headers: HeaderMap,
    Json(mut request): Json<StartUpdateRequest>,
) -> (StatusCode, Json<Value>) {
    let idempotency = match idempotency_key(&headers) {
        Ok(key) => key,
        Err(response) => return response,
    };
    let deadline = match deadline_seconds(request.deadline_seconds) {
        Ok(deadline) => deadline,
        Err(response) => return response,
    };
    let host = match ssh_trust::normalize_host(&request.host) {
        Ok(host) => host,
        Err(_) => return error(StatusCode::BAD_REQUEST, "host is invalid"),
    };
    request.host = host;
    if ssh_trust::validate_port(request.port).is_err() {
        return error(StatusCode::BAD_REQUEST, "port must be between 1 and 65535");
    }

    let (previous_version, os, protocol_version, arch) =
        match target_metadata(&state.pool, agent_id).await {
            Ok(metadata) => metadata,
            Err(response) => return response,
        };
    if previous_version == request.version {
        return error(
            StatusCode::CONFLICT,
            "agent already reports the requested release version",
        );
    }
    let Some((platform, architecture)) = platform_arch(&os, &arch) else {
        return error(
            StatusCode::CONFLICT,
            "agent platform or architecture is incompatible",
        );
    };
    let (repository, release) = match verified_release(&state.pool, &request.version).await {
        Ok(result) => result,
        Err(response) => return response,
    };
    let artifact = match repository.artifact_for(&release, platform, architecture, protocol_version)
    {
        Ok(artifact) => artifact,
        Err(repo_error) => {
            let (status, message) = repository_error(&repo_error);
            return error(status, message);
        }
    };
    if repository.read_artifact(&release, &artifact).is_err() {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "release artifact changed or is invalid",
        );
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "could not start update"),
    };
    if let Err(response) =
        validate_device_and_credential(&mut transaction, request.device_id, request.credential_id)
            .await
    {
        return response;
    }
    let active: Option<(Uuid,)> = match sqlx::query_as(
        "select id from agent_update_operations where agent_id = $1 \
         and state in ('pending', 'verifying', 'installing', 'restarting') \
         and not exists (select 1 from jobs where jobs.id = agent_update_operations.job_id \
                         and jobs.job_type = $2 and jobs.idempotency_key = $3) \
         limit 1",
    )
    .bind(agent_id)
    .bind(UPDATE_JOB_TYPE)
    .bind(idempotency)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(active) => active,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not inspect update state",
            );
        }
    };
    if active.is_some() {
        return error(
            StatusCode::CONFLICT,
            "agent already has an update in progress",
        );
    }
    let (job_id, operation_id) = match enqueue_update_in_transaction(
        &mut transaction,
        idempotency,
        EnqueueUpdate {
            request: &request,
            actor_user_id: Some(actor_user_id),
            agent_id,
            previous_version: &previous_version,
            platform,
            architecture,
            deadline,
        },
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not enqueue update",
            );
        }
    };
    if Recorder::record_audit(
        &mut transaction,
        Some(actor_user_id),
        "operator",
        "agent.update.requested",
        Some("agents"),
        Some(agent_id),
        "success",
        Some(json!({ "job_id": job_id, "operation_id": operation_id, "version": request.version })),
    )
    .await
    .is_err()
    {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not record update audit",
        );
    }
    if transaction.commit().await.is_err() {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "could not commit update");
    }
    (
        StatusCode::ACCEPTED,
        Json(json!({ "job_id": job_id, "operation_id": operation_id, "status": "pending" })),
    )
}

/// GET /api/v1/agents/:id/updates
pub async fn list_updates(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let rows = sqlx::query(
        "select id, job_id, target_version, previous_version, state, progress, \
                last_error, rollback_reason, deadline_at, started_at, finished_at, created_at, updated_at \
         from agent_update_operations where agent_id = $1 order by created_at desc limit 50",
    )
    .bind(agent_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "could not read update history"))?;
    let items = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.try_get::<Uuid, _>("id").ok(),
                "job_id": row.try_get::<Option<Uuid>, _>("job_id").ok().flatten(),
                "target_version": row.try_get::<String, _>("target_version").unwrap_or_default(),
                "previous_version": row.try_get::<Option<String>, _>("previous_version").ok().flatten(),
                "state": row.try_get::<String, _>("state").unwrap_or_default(),
                "progress": row.try_get::<Value, _>("progress").unwrap_or_else(|_| json!({})),
                "last_error": row.try_get::<Option<String>, _>("last_error").ok().flatten(),
                "rollback_reason": row.try_get::<Option<String>, _>("rollback_reason").ok().flatten(),
                "deadline_at": row.try_get::<Option<time::OffsetDateTime>, _>("deadline_at").ok().flatten(),
                "started_at": row.try_get::<Option<time::OffsetDateTime>, _>("started_at").ok().flatten(),
                "finished_at": row.try_get::<Option<time::OffsetDateTime>, _>("finished_at").ok().flatten(),
                "created_at": row.try_get::<time::OffsetDateTime, _>("created_at").ok(),
                "updated_at": row.try_get::<time::OffsetDateTime, _>("updated_at").ok(),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

/// GET /api/v1/agents/:id/compliance
///
/// This is intentionally a server-side projection rather than requiring the
/// UI to reproduce release selection and rollout rules.
pub async fn compliance(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (current_version, os, protocol_version, arch) =
        target_metadata(&state.pool, agent_id).await?;
    let policy = sqlx::query(
        "select mode, channel, pinned_version, rollout_percent from agent_update_policies \
         where agent_id = $1",
    )
    .bind(agent_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not read update policy",
        )
    })?;
    let mode = policy
        .as_ref()
        .and_then(|row| row.try_get::<String, _>("mode").ok())
        .unwrap_or_else(|| "manual".to_string());
    let channel = policy
        .as_ref()
        .and_then(|row| row.try_get::<String, _>("channel").ok())
        .unwrap_or_else(|| "stable".to_string());
    let pinned_version = policy.as_ref().and_then(|row| {
        row.try_get::<Option<String>, _>("pinned_version")
            .ok()
            .flatten()
    });
    let rollout_percent = policy
        .as_ref()
        .and_then(|row| row.try_get::<i32, _>("rollout_percent").ok())
        .unwrap_or(100);
    let channel_kind = channel.parse::<release::ReleaseChannel>().map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "stored update policy has an invalid release channel",
        )
    })?;

    let Some((platform, architecture)) = platform_arch(&os, &arch) else {
        return Ok(Json(json!({
            "agent_id": agent_id,
            "current_version": current_version,
            "status": "incompatible",
            "reason": "agent platform or architecture is unsupported",
            "mode": mode,
            "channel": channel,
            "pinned_version": pinned_version,
            "rollout_percent": rollout_percent,
        })));
    };

    let repository = ReleaseRepository::from_environment().map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "verified release repository is unavailable",
        )
    })?;
    let candidate = if let Some(version) = pinned_version.as_deref() {
        let release = repository.load(version).map_err(|_| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "pinned release is unavailable or invalid",
            )
        })?;
        repository
            .artifact_for(&release, platform, architecture, protocol_version)
            .map_err(|_| error(StatusCode::CONFLICT, "pinned release is incompatible"))?;
        Some(version.to_string())
    } else {
        repository
            .latest_compatible_for_channel(platform, architecture, protocol_version, channel_kind)
            .ok()
            .map(|(release, _)| release.manifest.version)
    };

    let latest_operation = sqlx::query(
        "select target_version, state, progress, last_error from agent_update_operations \
         where agent_id = $1 order by created_at desc limit 1",
    )
    .bind(agent_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not read update state",
        )
    })?;
    let operation_state = latest_operation
        .as_ref()
        .and_then(|row| row.try_get::<String, _>("state").ok());
    let active = matches!(
        operation_state.as_deref(),
        Some("pending" | "verifying" | "installing" | "restarting")
    );
    let rollout_deferred = mode == "automatic"
        && candidate
            .as_deref()
            .is_some_and(|_| !rollout_allows(agent_id, rollout_percent));
    let status = if active {
        "updating"
    } else if matches!(operation_state.as_deref(), Some("rolled_back")) {
        "rolled_back"
    } else if matches!(operation_state.as_deref(), Some("failed")) {
        "failed"
    } else if candidate.is_none() {
        "incompatible"
    } else if pinned_version.is_some() {
        "pinned"
    } else if rollout_deferred || candidate.as_deref() != Some(current_version.as_str()) {
        "update_available"
    } else {
        "current"
    };
    Ok(Json(json!({
        "agent_id": agent_id,
        "current_version": current_version,
        "target_version": candidate,
        "status": status,
        "mode": mode,
        "channel": channel,
        "pinned_version": pinned_version,
        "rollout_percent": rollout_percent,
        "rollout_deferred": rollout_deferred,
        "platform": platform,
        "architecture": architecture,
        "protocol_version": protocol_version,
        "operation": latest_operation.map(|row| json!({
            "target_version": row.try_get::<String, _>("target_version").unwrap_or_default(),
            "state": row.try_get::<String, _>("state").unwrap_or_default(),
            "progress": row.try_get::<Value, _>("progress").unwrap_or_else(|_| json!({})),
            "last_error": row.try_get::<Option<String>, _>("last_error").ok().flatten(),
        })),
    })))
}

/// GET /api/v1/agents/:id/update-policy
pub async fn get_policy(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let row = sqlx::query(
        "select agent_id, mode, channel, pinned_version, rollout_percent, device_id, host, port, \
                credential_id, repair_on_failure, updated_by, created_at, updated_at \
         from agent_update_policies where agent_id = $1",
    )
    .bind(agent_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not read update policy",
        )
    })?;
    let Some(row) = row else {
        return Ok(Json(json!({
            "agent_id": agent_id,
            "mode": "manual",
            "channel": "stable",
            "rollout_percent": 100,
        })));
    };
    Ok(Json(json!({
        "agent_id": row.try_get::<Uuid, _>("agent_id").ok(),
        "mode": row.try_get::<String, _>("mode").unwrap_or_default(),
        "channel": row.try_get::<String, _>("channel").unwrap_or_default(),
        "pinned_version": row.try_get::<Option<String>, _>("pinned_version").ok().flatten(),
        "rollout_percent": row.try_get::<i32, _>("rollout_percent").unwrap_or(100),
        "device_id": row.try_get::<Uuid, _>("device_id").ok(),
        "host": row.try_get::<String, _>("host").unwrap_or_default(),
        "port": row.try_get::<i32, _>("port").unwrap_or_default(),
        "credential_id": row.try_get::<Uuid, _>("credential_id").ok(),
        "repair_on_failure": row.try_get::<bool, _>("repair_on_failure").unwrap_or(true),
        "updated_by": row.try_get::<Option<Uuid>, _>("updated_by").ok().flatten(),
        "created_at": row.try_get::<time::OffsetDateTime, _>("created_at").ok(),
        "updated_at": row.try_get::<time::OffsetDateTime, _>("updated_at").ok(),
    })))
}

/// PUT /api/v1/agents/:id/update-policy
pub async fn put_policy(
    State(state): State<AppState>,
    Extension(CurrentUser(actor_user_id)): Extension<CurrentUser>,
    Path(agent_id): Path<Uuid>,
    Json(request): Json<UpdatePolicyRequest>,
) -> (StatusCode, Json<Value>) {
    if !matches!(request.mode.as_str(), "manual" | "notify" | "automatic") {
        return error(
            StatusCode::BAD_REQUEST,
            "mode must be manual, notify, or automatic",
        );
    }
    if !matches!(request.channel.as_str(), "stable" | "canary") {
        return error(StatusCode::BAD_REQUEST, "channel must be stable or canary");
    }
    if !(0..=100).contains(&request.rollout_percent) {
        return error(
            StatusCode::BAD_REQUEST,
            "rollout_percent must be between 0 and 100",
        );
    }
    let host = match ssh_trust::normalize_host(&request.host) {
        Ok(host) => host,
        Err(_) => return error(StatusCode::BAD_REQUEST, "host is invalid"),
    };
    if ssh_trust::validate_port(request.port).is_err() {
        return error(StatusCode::BAD_REQUEST, "port must be between 1 and 65535");
    }
    let (_, os, protocol_version, arch) = match target_metadata(&state.pool, agent_id).await {
        Ok(metadata) => metadata,
        Err(response) => return response,
    };
    if let Some(version) = request.pinned_version.as_deref() {
        let Ok((repository, release)) = verified_release(&state.pool, version).await else {
            return error(
                StatusCode::CONFLICT,
                "pinned release is not available and verified",
            );
        };
        let Some((platform, architecture)) = platform_arch(&os, &arch) else {
            return error(
                StatusCode::CONFLICT,
                "agent platform or architecture is incompatible",
            );
        };
        if repository
            .artifact_for(&release, platform, architecture, protocol_version)
            .is_err()
        {
            return error(
                StatusCode::CONFLICT,
                "pinned release is incompatible with the agent",
            );
        }
    }
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not start policy update",
            );
        }
    };
    if let Err(response) =
        validate_device_and_credential(&mut transaction, request.device_id, request.credential_id)
            .await
    {
        return response;
    }
    if sqlx::query(
        "insert into agent_update_policies \
             (agent_id, mode, channel, pinned_version, rollout_percent, device_id, host, port, credential_id, repair_on_failure, updated_by) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
         on conflict (agent_id) do update set mode = excluded.mode, channel = excluded.channel, \
             pinned_version = excluded.pinned_version, rollout_percent = excluded.rollout_percent, \
             device_id = excluded.device_id, host = excluded.host, port = excluded.port, \
             credential_id = excluded.credential_id, repair_on_failure = excluded.repair_on_failure, \
             updated_by = excluded.updated_by, updated_at = now()",
    )
    .bind(agent_id)
    .bind(&request.mode)
    .bind(&request.channel)
    .bind(&request.pinned_version)
    .bind(request.rollout_percent)
    .bind(request.device_id)
    .bind(host)
    .bind(request.port)
    .bind(request.credential_id)
    .bind(request.repair_on_failure)
    .bind(actor_user_id)
    .execute(&mut *transaction)
    .await
    .is_err()
    {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "could not save update policy");
    }
    if Recorder::record_audit(
        &mut transaction,
        Some(actor_user_id),
        "operator",
        "agent.update_policy.changed",
        Some("agents"),
        Some(agent_id),
        "success",
        Some(json!({ "mode": request.mode, "channel": request.channel, "pinned_version": request.pinned_version, "rollout_percent": request.rollout_percent })),
    )
    .await
    .is_err()
    {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "could not record policy audit");
    }
    if transaction.commit().await.is_err() {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not commit update policy",
        );
    }
    (
        StatusCode::OK,
        Json(json!({ "agent_id": agent_id, "status": "saved" })),
    )
}

pub async fn handle_update(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    payload: Value,
) -> anyhow::Result<JobOutcome> {
    let agent_id = payload_uuid(&payload, "agent_id")?;
    let device_id = payload_uuid(&payload, "device_id")?;
    let credential_id = payload_uuid(&payload, "credential_id")?;
    let target_version = payload_string(&payload, "target_version")?;
    let previous_version = payload
        .get("previous_version")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let host = payload_string(&payload, "host")?;
    let port = payload_i32(&payload, "port")?;
    let actor_user_id = payload
        .get("actor_user_id")
        .and_then(Value::as_str)
        .map(Uuid::parse_str)
        .transpose()?;
    let platform = payload_string(&payload, "platform")?;
    let architecture = payload_string(&payload, "architecture")?;
    let deadline_seconds = payload
        .get("deadline_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_DEADLINE_SECONDS)
        .clamp(10, MAX_DEADLINE_SECONDS);
    let repair_on_failure = payload
        .get("repair_on_failure")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    set_operation_state(
        pool,
        job_id,
        "verifying",
        json!({ "phase": "verifying", "version": target_version }),
        None,
        false,
    )
    .await?;
    jobs::heartbeat(
        pool,
        job_id,
        worker_id,
        60,
        Some(json!({ "phase": "verifying", "version": target_version })),
    )
    .await?;

    let repository = match ReleaseRepository::from_environment() {
        Ok(repository) => repository,
        Err(error) => {
            mark_update_failed(pool, job_id, &error.to_string()).await?;
            return Err(error.into());
        }
    };
    let release = match repository.load(&target_version) {
        Ok(release) => release,
        Err(error) => {
            mark_update_failed(pool, job_id, &error.to_string()).await?;
            return Err(error.into());
        }
    };
    let protocol_version: i32 = match sqlx::query_scalar(
        "select protocol_version from agents where id = $1 and revoked_at is null",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(protocol_version)) => protocol_version,
        Ok(None) => {
            let error = anyhow::anyhow!("agent is not enrolled or has been revoked");
            mark_update_failed(pool, job_id, &error.to_string()).await?;
            return Err(error);
        }
        Err(error) => {
            mark_update_failed(pool, job_id, &error.to_string()).await?;
            return Err(error.into());
        }
    };
    let artifact = match repository.artifact_for(
        &release,
        &platform,
        &architecture,
        u32::try_from(protocol_version).unwrap_or(0),
    ) {
        Ok(artifact) => artifact,
        Err(error) => {
            mark_update_failed(pool, job_id, &error.to_string()).await?;
            return Err(error.into());
        }
    };
    let binary = match repository.read_artifact(&release, &artifact) {
        Ok(binary) => binary,
        Err(error) => {
            mark_update_failed(pool, job_id, &error.to_string()).await?;
            return Err(error.into());
        }
    };
    set_operation_state(
        pool,
        job_id,
        "installing",
        json!({ "phase": "installing", "version": target_version, "bytes": binary.len() }),
        None,
        true,
    )
    .await?;
    jobs::heartbeat(
        pool,
        job_id,
        worker_id,
        60,
        Some(json!({ "phase": "installing", "version": target_version })),
    )
    .await?;
    set_operation_state(
        pool,
        job_id,
        "restarting",
        json!({ "phase": "restarting", "version": target_version }),
        None,
        true,
    )
    .await?;

    let store = match CredentialStore::from_environment(pool.clone()) {
        Ok(store) => store,
        Err(error) => {
            mark_update_failed(pool, job_id, &error.to_string()).await?;
            return Err(error.into());
        }
    };
    let result = ssh_install::update_or_rollback(
        pool,
        &store,
        ssh_install::AgentUpdateRequest {
            device_id,
            agent_id,
            host,
            port,
            credential_id,
            actor_user_id,
            target_version: target_version.clone(),
            previous_version,
            expected_platform: platform,
            expected_architecture: architecture,
            binary,
            check_in_deadline: Duration::from_secs(deadline_seconds as u64),
            repair_on_failure,
        },
    )
    .await;
    match result {
        Ok(result) => {
            let state = match result.state {
                ssh_install::AgentUpdateState::Succeeded => "succeeded",
                ssh_install::AgentUpdateState::RolledBack
                | ssh_install::AgentUpdateState::Repaired => "rolled_back",
            };
            let rollback_reason = match result.state {
                ssh_install::AgentUpdateState::Succeeded => None,
                ssh_install::AgentUpdateState::RolledBack => {
                    Some("target agent did not check in; previous binary was restored".to_string())
                }
                ssh_install::AgentUpdateState::Repaired => {
                    Some("target and rollback check-ins failed; SSH repair completed".to_string())
                }
            };
            let progress = json!({
                "phase": state,
                "version": target_version,
                "platform": result.platform,
                "architecture": result.architecture,
                "repair_attempted": matches!(result.state, ssh_install::AgentUpdateState::Repaired),
            });
            set_operation_state(pool, job_id, state, progress, None, true).await?;
            if let Some(reason) = rollback_reason {
                sqlx::query(
                    "update agent_update_operations set rollback_reason = $2, updated_at = now() \
                     where job_id = $1",
                )
                .bind(job_id)
                .bind(reason)
                .execute(pool)
                .await?;
            }
            Ok(JobOutcome::Completed)
        }
        Err(error) => {
            let message = error.to_string();
            set_operation_state(
                pool,
                job_id,
                "failed",
                json!({ "phase": "failed" }),
                Some(&message),
                true,
            )
            .await?;
            Err(error.into())
        }
    }
}

pub async fn reconcile(
    pool: &PgPool,
    _job_id: Uuid,
    _worker_id: &str,
) -> anyhow::Result<JobOutcome> {
    let repository = match ReleaseRepository::from_environment() {
        Ok(repository) => repository,
        Err(error) => {
            tracing::warn!(error = %error, "automatic agent update reconciliation is disabled");
            return Ok(JobOutcome::Completed);
        }
    };
    let rows = sqlx::query(
        "select p.agent_id, p.channel, p.pinned_version, p.rollout_percent, p.device_id, \
                p.host, p.port, p.credential_id, p.repair_on_failure, a.agent_version, \
                a.os, a.arch, a.protocol_version \
         from agent_update_policies p join agents a on a.id = p.agent_id \
         where p.mode = 'automatic' and a.revoked_at is null",
    )
    .fetch_all(pool)
    .await?;
    for row in rows {
        let agent_id: Uuid = row.get("agent_id");
        let Some((platform, architecture)) = platform_arch(
            row.get::<String, _>("os").as_str(),
            row.get::<String, _>("arch").as_str(),
        ) else {
            continue;
        };
        let protocol_version: u32 =
            u32::try_from(row.get::<i32, _>("protocol_version")).unwrap_or(0);
        let Ok(channel) = row
            .get::<String, _>("channel")
            .parse::<release::ReleaseChannel>()
        else {
            continue;
        };
        let target = if let Some(pinned) = row.get::<Option<String>, _>("pinned_version") {
            match repository.load(&pinned) {
                Ok(release)
                    if repository
                        .artifact_for(&release, platform, architecture, protocol_version)
                        .is_ok() =>
                {
                    pinned
                }
                _ => continue,
            }
        } else {
            let Ok((release, _)) = repository.latest_compatible_for_channel(
                platform,
                architecture,
                protocol_version,
                channel,
            ) else {
                continue;
            };
            release.manifest.version
        };
        let current = row.get::<String, _>("agent_version");
        if current == target || !is_newer_version(&target, &current) {
            continue;
        }
        if !rollout_allows(agent_id, row.get::<i32, _>("rollout_percent")) {
            continue;
        }
        let active: bool = sqlx::query_scalar(
            "select exists(select 1 from agent_update_operations \
             where agent_id = $1 and state in ('pending', 'verifying', 'installing', 'restarting'))",
        )
        .bind(agent_id)
        .fetch_one(pool)
        .await?;
        if active {
            continue;
        }
        let request = StartUpdateRequest {
            device_id: row.get("device_id"),
            host: row.get("host"),
            port: row.get("port"),
            credential_id: row.get("credential_id"),
            version: target.clone(),
            deadline_seconds: Some(DEFAULT_DEADLINE_SECONDS),
            repair_on_failure: row.get("repair_on_failure"),
        };
        let mut transaction = pool.begin().await?;
        if validate_device_and_credential(
            &mut transaction,
            request.device_id,
            request.credential_id,
        )
        .await
        .is_err()
        {
            continue;
        }
        let idempotency = format!("automatic:{agent_id}:{target}");
        let (job_id, _) = enqueue_update_in_transaction(
            &mut transaction,
            &idempotency,
            EnqueueUpdate {
                request: &request,
                actor_user_id: None,
                agent_id,
                previous_version: &current,
                platform,
                architecture,
                deadline: DEFAULT_DEADLINE_SECONDS,
            },
        )
        .await?;
        transaction.commit().await?;
        tracing::info!(%agent_id, %job_id, version = %target, "enqueued automatic agent update");
    }
    Ok(JobOutcome::Completed)
}

async fn set_operation_state(
    pool: &PgPool,
    job_id: Uuid,
    state: &str,
    progress: Value,
    last_error: Option<&str>,
    set_started: bool,
) -> sqlx::Result<()> {
    sqlx::query(
        "update agent_update_operations set state = $2, progress = $3, last_error = $4, \
         started_at = case when $5 then coalesce(started_at, now()) else started_at end, \
         finished_at = case when $2 in ('succeeded', 'failed', 'rolled_back') then now() else finished_at end, \
         updated_at = now() where job_id = $1",
    )
    .bind(job_id)
    .bind(state)
    .bind(progress)
    .bind(last_error)
    .bind(set_started)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_update_failed(pool: &PgPool, job_id: Uuid, message: &str) -> anyhow::Result<()> {
    set_operation_state(
        pool,
        job_id,
        "failed",
        json!({ "phase": "failed" }),
        Some(message),
        true,
    )
    .await?;
    Ok(())
}

fn payload_uuid(payload: &Value, field: &str) -> anyhow::Result<Uuid> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("update payload has no {field}"))
        .and_then(|value| Uuid::parse_str(value).map_err(Into::into))
}

fn payload_string(payload: &Value, field: &str) -> anyhow::Result<String> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow::anyhow!("update payload has no {field}"))
}

fn payload_i32(payload: &Value, field: &str) -> anyhow::Result<i32> {
    let value = payload
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("update payload has no {field}"))?;
    i32::try_from(value).map_err(Into::into)
}

fn rollout_allows(agent_id: Uuid, percent: i32) -> bool {
    if percent >= 100 {
        return true;
    }
    if percent <= 0 {
        return false;
    }
    let mut digest = Sha256::new();
    digest.update(agent_id.as_bytes());
    let digest = digest.finalize();
    let bucket = u16::from_be_bytes([digest[0], digest[1]]) % 100;
    i32::from(bucket) < percent
}

fn is_newer_version(candidate: &str, current: &str) -> bool {
    match (Version::parse(candidate), Version::parse(current)) {
        (Ok(candidate), Ok(current)) => candidate > current,
        (Ok(_), Err(_)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_architecture_mapping_is_strict() {
        assert_eq!(platform_arch("linux", "x86_64"), Some(("linux", "amd64")));
        assert_eq!(platform_arch("linux", "aarch64"), Some(("linux", "arm64")));
        assert_eq!(platform_arch("windows", "x86_64"), None);
        assert_eq!(platform_arch("linux", "riscv64"), None);
    }

    #[test]
    fn version_comparison_handles_numeric_components_and_prereleases() {
        assert!(is_newer_version("1.10.0", "1.9.9"));
        assert!(is_newer_version("1.2.0", "1.2.0-rc1"));
        assert!(is_newer_version("1.2.0-rc.10", "1.2.0-rc.2"));
        assert!(!is_newer_version("1.2.0-rc1", "1.2.0"));
        assert!(!is_newer_version("1.2.0", "1.2.0"));
    }

    #[test]
    fn rollout_percentage_is_deterministic() {
        let agent_id = Uuid::from_u128(1);
        assert!(!rollout_allows(agent_id, 0));
        assert!(rollout_allows(agent_id, 100));
        assert_eq!(rollout_allows(agent_id, 25), rollout_allows(agent_id, 25));
    }
}
