//! Session-authenticated, metadata-only credential API.
//!
//! Secret values are accepted only by create/update requests and are never
//! included in a response. Decryption for SSH work happens in a worker, not
//! in an HTTP handler.

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::auth_mw::CurrentUser;
use crate::credentials::{
    CredentialError, CredentialScope, CredentialSecret, CredentialStore, CredentialUpdate,
    NewCredential,
};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub cursor: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SecretRequest {
    SshPrivateKey {
        username: String,
        private_key_pem: String,
        passphrase: Option<String>,
    },
    SshPassword {
        username: String,
        password: String,
    },
}

impl From<SecretRequest> for CredentialSecret {
    fn from(value: SecretRequest) -> Self {
        match value {
            SecretRequest::SshPrivateKey {
                username,
                private_key_pem,
                passphrase,
            } => Self::SshPrivateKey {
                username: Zeroizing::new(username),
                private_key_pem: Zeroizing::new(private_key_pem),
                passphrase: passphrase.map(Zeroizing::new),
            },
            SecretRequest::SshPassword { username, password } => Self::SshPassword {
                username: Zeroizing::new(username),
                password: Zeroizing::new(password),
            },
        }
    }
}

#[derive(Deserialize)]
pub struct CreateRequest {
    pub name: String,
    pub scope: CredentialScope,
    pub secret: SecretRequest,
}

#[derive(Deserialize)]
pub struct UpdateRequest {
    pub name: Option<String>,
    pub scope: Option<CredentialScope>,
    pub secret: Option<SecretRequest>,
    pub expected_version: i32,
}

fn error_response(error: CredentialError) -> (StatusCode, Json<Value>) {
    let status = match error {
        CredentialError::InvalidInput(_)
        | CredentialError::InvalidMasterKeyEncoding
        | CredentialError::InvalidMasterKeyLength
        | CredentialError::PayloadTooLarge => StatusCode::BAD_REQUEST,
        CredentialError::MasterKeyMissing | CredentialError::MasterKeyFile(_) => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        CredentialError::Unavailable => StatusCode::NOT_FOUND,
        CredentialError::InvalidCiphertext | CredentialError::InvalidSecret => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        CredentialError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let message = match status {
        StatusCode::BAD_REQUEST => error.to_string(),
        StatusCode::NOT_FOUND => "credential not found".to_string(),
        StatusCode::SERVICE_UNAVAILABLE => "credential vault is not configured".to_string(),
        _ => "credential operation failed".to_string(),
    };
    (status, Json(json!({ "error": message })))
}

/// GET /api/v1/credentials
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let store = CredentialStore::from_environment(state.pool.clone()).map_err(error_response)?;
    let items = store
        .list(query.limit.unwrap_or(50), query.cursor)
        .await
        .map_err(error_response)?;
    Ok(Json(json!({ "items": items })))
}

/// GET /api/v1/credentials/:id
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let store = CredentialStore::from_environment(state.pool.clone()).map_err(error_response)?;
    let Some(metadata) = store.get(id).await.map_err(error_response)? else {
        return Err(error_response(CredentialError::Unavailable));
    };
    Ok(Json(json!(metadata)))
}

/// POST /api/v1/credentials
pub async fn create(
    State(state): State<AppState>,
    Extension(CurrentUser(user_id)): Extension<CurrentUser>,
    Json(request): Json<CreateRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let store = CredentialStore::from_environment(state.pool.clone()).map_err(error_response)?;
    let metadata = store
        .create(NewCredential {
            name: request.name,
            scope: request.scope,
            secret: request.secret.into(),
            created_by: Some(user_id),
        })
        .await
        .map_err(error_response)?;
    Ok((StatusCode::CREATED, Json(json!(metadata))))
}

/// PATCH /api/v1/credentials/:id
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(CurrentUser(user_id)): Extension<CurrentUser>,
    Json(request): Json<UpdateRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let store = CredentialStore::from_environment(state.pool.clone()).map_err(error_response)?;
    let metadata = store
        .update(
            id,
            Some(user_id),
            CredentialUpdate {
                name: request.name,
                scope: request.scope,
                secret: request.secret.map(Into::into),
                expected_version: request.expected_version,
            },
        )
        .await
        .map_err(error_response)?;
    Ok(Json(json!(metadata)))
}

/// DELETE /api/v1/credentials/:id
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(CurrentUser(user_id)): Extension<CurrentUser>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let store = CredentialStore::from_environment(state.pool.clone()).map_err(error_response)?;
    let deleted = store
        .delete(id, Some(user_id))
        .await
        .map_err(error_response)?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(error_response(CredentialError::Unavailable))
    }
}
