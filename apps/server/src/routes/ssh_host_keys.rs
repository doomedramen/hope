//! Metadata-only SSH host-key trust controls.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::ssh_trust::{self, SshTrustError};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub device_id: Option<Uuid>,
}

fn error(error: SshTrustError) -> (StatusCode, Json<Value>) {
    let status = match &error {
        SshTrustError::InvalidHost
        | SshTrustError::InvalidPort
        | SshTrustError::InvalidKeyType
        | SshTrustError::InvalidFingerprint
        | SshTrustError::InvalidState => StatusCode::BAD_REQUEST,
        SshTrustError::NotFound => StatusCode::NOT_FOUND,
        SshTrustError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let message = match status {
        StatusCode::BAD_REQUEST => error.to_string(),
        StatusCode::NOT_FOUND => "SSH host-key record not found".to_string(),
        _ => "SSH host-key operation failed".to_string(),
    };
    (status, Json(json!({ "error": message })))
}

/// GET /api/v1/ssh-host-keys
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let rows = match sqlx::query(
        "select id, device_id, host, port, key_type, fingerprint_sha256, \
                previous_fingerprint_sha256, state, first_seen_at, last_seen_at, \
                trusted_at, changed_at, revoked_at, version \
         from ssh_host_keys where ($1::uuid is null or device_id = $1) \
         order by last_seen_at desc limit 100",
    )
    .bind(query.device_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(_) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "SSH host-key operation failed" })),
            ));
        }
    };
    let items = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.try_get::<Uuid, _>("id").ok(),
                "device_id": row.try_get::<Option<Uuid>, _>("device_id").ok().flatten(),
                "host": row.try_get::<String, _>("host").unwrap_or_default(),
                "port": row.try_get::<i32, _>("port").unwrap_or_default(),
                "key_type": row.try_get::<String, _>("key_type").unwrap_or_default(),
                "fingerprint_sha256": row.try_get::<String, _>("fingerprint_sha256").unwrap_or_default(),
                "previous_fingerprint_sha256": row.try_get::<Option<String>, _>("previous_fingerprint_sha256").ok().flatten(),
                "state": row.try_get::<String, _>("state").unwrap_or_default(),
                "first_seen_at": row.try_get::<time::OffsetDateTime, _>("first_seen_at").ok(),
                "last_seen_at": row.try_get::<time::OffsetDateTime, _>("last_seen_at").ok(),
                "trusted_at": row.try_get::<Option<time::OffsetDateTime>, _>("trusted_at").ok().flatten(),
                "changed_at": row.try_get::<Option<time::OffsetDateTime>, _>("changed_at").ok().flatten(),
                "revoked_at": row.try_get::<Option<time::OffsetDateTime>, _>("revoked_at").ok().flatten(),
                "version": row.try_get::<i32, _>("version").unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

/// GET /api/v1/ssh-host-keys/:id
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let metadata = ssh_trust::get(&state.pool, id)
        .await
        .map_err(error)?
        .ok_or_else(|| error(SshTrustError::NotFound))?;
    Ok(Json(json!(metadata)))
}

/// POST /api/v1/ssh-host-keys/:id/trust
pub async fn trust(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let metadata = ssh_trust::trust(&state.pool, id, Some(user))
        .await
        .map_err(error)?;
    Ok(Json(json!(metadata)))
}

/// POST /api/v1/ssh-host-keys/:id/revoke
pub async fn revoke(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let metadata = ssh_trust::revoke(&state.pool, id, Some(user))
        .await
        .map_err(error)?;
    Ok(Json(json!(metadata)))
}
