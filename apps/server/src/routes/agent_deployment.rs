//! Operator-authenticated agent install and repair job endpoints.
//!
//! The request contains only a credential ID. Secret material is loaded by
//! the worker from the vault and never enters the job payload or response.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::inventory::events::Recorder;
use crate::ssh_trust;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct DeploymentRequest {
    pub host: String,
    pub port: i32,
    pub credential_id: Uuid,
    #[serde(default)]
    pub disassociate_after_enrollment: bool,
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

async fn enqueue(
    state: AppState,
    user: CurrentUser,
    device_id: Uuid,
    headers: HeaderMap,
    request: DeploymentRequest,
    repair: bool,
) -> (StatusCode, Json<Value>) {
    let key = match idempotency_key(&headers) {
        Ok(key) => key,
        Err(response) => return response,
    };
    let host = match ssh_trust::normalize_host(&request.host) {
        Ok(host) => host,
        Err(_) => return error(StatusCode::BAD_REQUEST, "host is invalid"),
    };
    if ssh_trust::validate_port(request.port).is_err() {
        return error(StatusCode::BAD_REQUEST, "port must be between 1 and 65535");
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "could not start job"),
    };
    let job_type = if repair {
        "agent.repair"
    } else {
        "agent.install"
    };

    // Hold a key-share lock until commit so a device cannot be deleted between
    // validation and the foreign-key-backed association/job transaction.
    match sqlx::query("select id from devices where id = $1 for key share")
        .bind(device_id)
        .fetch_optional(&mut *transaction)
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::NOT_FOUND, "device not found"),
        Err(_) => {
            return error(StatusCode::INTERNAL_SERVER_ERROR, "could not verify device");
        }
    }

    // Insert only an active SSH credential. The INSERT ... SELECT keeps the
    // existence check and association atomic, while fetching metadata never
    // loads secret_ciphertext into the handler.
    match sqlx::query(
        "insert into credential_associations (credential_id, device_id, purpose) \
         select $1, $2, 'agent_install' from credentials \
         where id = $1 and deleted_at is null and revoked_at is null \
           and kind in ('ssh_private_key', 'ssh_password') \
         on conflict (credential_id, device_id, purpose) do update set disassociated_at = null \
         returning id",
    )
    .bind(request.credential_id)
    .bind(device_id)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::NOT_FOUND, "credential not found"),
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not associate credential",
            );
        }
    }
    let payload = json!({
        "device_id": device_id,
        "host": host,
        "port": request.port,
        "credential_id": request.credential_id,
        "actor_user_id": user.0,
        "repair": repair,
        "disassociate_after_enrollment": request.disassociate_after_enrollment,
    });
    let job_id = match jobs::enqueue_in(&mut transaction, job_type, key, payload).await {
        Ok(job_id) => job_id,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "could not enqueue job"),
    };
    if Recorder::record_audit(
        &mut transaction,
        Some(user.0),
        "operator",
        job_type,
        Some("devices"),
        Some(device_id),
        "success",
        Some(json!({ "job_id": job_id, "host": host, "port": request.port })),
    )
    .await
    .is_err()
    {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not record deployment audit",
        );
    }
    if transaction.commit().await.is_err() {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "could not commit job");
    }
    (
        StatusCode::ACCEPTED,
        Json(json!({ "job_id": job_id, "status": "pending" })),
    )
}

/// POST /api/v1/devices/:id/agent-install
pub async fn install(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(device_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<DeploymentRequest>,
) -> (StatusCode, Json<Value>) {
    enqueue(state, user, device_id, headers, request, false).await
}

/// POST /api/v1/devices/:id/agent-repair
pub async fn repair(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(device_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<DeploymentRequest>,
) -> (StatusCode, Json<Value>) {
    enqueue(state, user, device_id, headers, request, true).await
}

/// GET /api/v1/jobs/:id — bounded job status without returning the payload.
pub async fn get_job(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    let row = sqlx::query(
        "select id, job_type, status, progress, attempts, max_attempts, run_at, \
                last_error, created_at, updated_at from jobs where id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await;
    match row {
        Ok(Some(row)) => (
            StatusCode::OK,
            Json(json!({
                "id": row.try_get::<Uuid, _>("id").unwrap_or(id),
                "job_type": row.try_get::<String, _>("job_type").unwrap_or_default(),
                "status": row.try_get::<String, _>("status").unwrap_or_default(),
                "progress": row.try_get::<Value, _>("progress").unwrap_or_else(|_| json!({})),
                "attempts": row.try_get::<i32, _>("attempts").unwrap_or_default(),
                "max_attempts": row.try_get::<i32, _>("max_attempts").unwrap_or_default(),
                "run_at": row.try_get::<time::OffsetDateTime, _>("run_at").ok(),
                "last_error": row.try_get::<Option<String>, _>("last_error").ok().flatten(),
                "created_at": row.try_get::<time::OffsetDateTime, _>("created_at").ok(),
                "updated_at": row.try_get::<time::OffsetDateTime, _>("updated_at").ok(),
            })),
        ),
        Ok(None) => error(StatusCode::NOT_FOUND, "job not found"),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "could not read job"),
    }
}
