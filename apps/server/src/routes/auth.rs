use argon2::password_hash::{PasswordHash, PasswordHasher, SaltString, rand_core::OsRng};
use argon2::{Argon2, PasswordVerifier};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use tower_sessions::Session;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct SetupRequest {
    pub email: String,
    pub password: String,
}

/// POST /api/v1/setup — bootstrap the single admin account (spec §13.2
/// first-run flow, step 1). Only succeeds when no user exists yet.
pub async fn setup(
    State(state): State<AppState>,
    Json(req): Json<SetupRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if req.email.trim().is_empty() || req.password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "email required and password must be at least 8 characters" })),
        );
    }

    let existing: (i64,) = match sqlx::query_as("select count(*) from users")
        .fetch_one(&state.pool)
        .await
    {
        Ok(row) => row,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            );
        }
    };

    if existing.0 > 0 {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "an admin account already exists" })),
        );
    }

    let salt = SaltString::generate(&mut OsRng);
    let hash = match Argon2::default().hash_password(req.password.as_bytes(), &salt) {
        Ok(hash) => hash.to_string(),
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            );
        }
    };

    let result = sqlx::query("insert into users (email, password_hash) values ($1, $2)")
        .bind(&req.email)
        .bind(&hash)
        .execute(&state.pool)
        .await;

    match result {
        Ok(_) => (StatusCode::CREATED, Json(json!({ "email": req.email }))),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// POST /api/v1/login — verify credentials and establish a session.
pub async fn login(
    State(state): State<AppState>,
    session: Session,
    Json(req): Json<LoginRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let row: Option<(String, String)> =
        match sqlx::query_as("select id::text, password_hash from users where email = $1")
            .bind(&req.email)
            .fetch_optional(&state.pool)
            .await
        {
            Ok(row) => row,
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": err.to_string() })),
                );
            }
        };

    let Some((user_id, password_hash)) = row else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid credentials" })),
        );
    };

    let parsed_hash = match PasswordHash::new(&password_hash) {
        Ok(hash) => hash,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "corrupt password hash" })),
            );
        }
    };

    if Argon2::default()
        .verify_password(req.password.as_bytes(), &parsed_hash)
        .is_err()
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid credentials" })),
        );
    }

    if let Err(err) = session.insert("user_id", user_id).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        );
    }

    (StatusCode::OK, Json(json!({ "status": "ok" })))
}
