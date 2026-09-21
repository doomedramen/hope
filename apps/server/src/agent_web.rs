//! Agent enrollment and authenticated WebSocket upgrade on the web origin.

use axum::Json;
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::agents;
use crate::gateway;
use crate::state::AppState;

const CHALLENGE_TTL_SECONDS: i64 = 60;

#[derive(Deserialize)]
pub struct EnrollmentRequest {
    token: String,
    public_key_hex: String,
    hostname: Option<String>,
}

#[derive(Serialize)]
struct EnrollmentResponse {
    agent_id: Uuid,
}

fn decode_key(encoded: &str) -> Result<VerifyingKey, StatusCode> {
    let bytes: [u8; 32] = hex::decode(encoded)
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .try_into()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| StatusCode::BAD_REQUEST)
}

pub async fn enroll(
    State(state): State<AppState>,
    Json(request): Json<EnrollmentRequest>,
) -> Response {
    let key = match decode_key(&request.public_key_hex) {
        Ok(key) => key,
        Err(status) => return status.into_response(),
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let consumed = sqlx::query(
        "update enrollment_tokens set used_at = now() where token_hash = $1 \
         and used_at is null and expires_at > now() returning token_hash",
    )
    .bind(agents::hash_token(&request.token))
    .fetch_optional(&mut *transaction)
    .await;
    match consumed {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    let fingerprint = hex::encode(Sha256::digest(key.to_bytes()));
    let inserted = sqlx::query(
        "insert into agents (cert_fingerprint, cert_serial, public_key_hex, hostname) \
         values ($1, 'web-key-v1', $2, $3) returning id",
    )
    .bind(&fingerprint)
    .bind(hex::encode(key.to_bytes()))
    .bind(request.hostname.as_deref())
    .fetch_one(&mut *transaction)
    .await;
    match inserted {
        Ok(row) => {
            if transaction.commit().await.is_err() {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            (
                StatusCode::CREATED,
                Json(EnrollmentResponse {
                    agent_id: row.get("id"),
                }),
            )
                .into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

pub async fn challenge(State(state): State<AppState>, Path(agent_id): Path<Uuid>) -> Response {
    let active = sqlx::query_scalar::<_, bool>(
        "select exists(select 1 from agents where id = $1 and revoked_at is null \
         and public_key_hex is not null)",
    )
    .bind(agent_id)
    .fetch_one(&state.pool)
    .await;
    if !matches!(active, Ok(true)) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let nonce = hex::encode(bytes);
    let nonce_hash = hex::encode(Sha256::digest(nonce.as_bytes()));
    let inserted = sqlx::query(
        "insert into agent_connection_challenges (nonce_hash, agent_id, expires_at) \
         values ($1, $2, now() + make_interval(secs => $3))",
    )
    .bind(nonce_hash)
    .bind(agent_id)
    .bind(CHALLENGE_TTL_SECONDS as i32)
    .execute(&state.pool)
    .await;
    if inserted.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    (StatusCode::OK, Json(serde_json::json!({ "nonce": nonce }))).into_response()
}

pub async fn connect(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let agent_id = match authenticate_connection(&state.pool, &headers).await {
        Ok(agent_id) => agent_id,
        Err(status) => return status.into_response(),
    };
    ws.on_upgrade(move |socket| gateway::handle_socket(socket, state.pool, agent_id))
}

async fn authenticate_connection(
    pool: &sqlx::PgPool,
    headers: &HeaderMap,
) -> Result<Uuid, StatusCode> {
    let get = |name| headers.get(name).and_then(|value| value.to_str().ok());
    let (Some(agent_id), Some(nonce), Some(signature_hex)) = (
        get("x-hope-agent-id").and_then(|id| Uuid::parse_str(id).ok()),
        get("x-hope-challenge"),
        get("x-hope-signature"),
    ) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    if nonce.len() != 64 || signature_hex.len() != 128 {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let nonce_hash = hex::encode(Sha256::digest(nonce.as_bytes()));
    let row = sqlx::query(
        "update agent_connection_challenges c set used_at = now() \
         from agents a where c.nonce_hash = $1 and c.agent_id = $2 \
         and c.used_at is null and c.expires_at > now() \
         and a.id = c.agent_id and a.revoked_at is null \
         returning a.public_key_hex",
    )
    .bind(nonce_hash)
    .bind(agent_id)
    .fetch_optional(pool)
    .await;
    let Some(row) = (match row {
        Ok(row) => row,
        Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
    }) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let Some(key_hex): Option<String> = row.get("public_key_hex") else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let Ok(key) = decode_key(&key_hex) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let Ok(signature_bytes) = hex::decode(signature_hex) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let Ok(signature) = Signature::from_slice(&signature_bytes) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let message = format!("hope-agent-connect-v1\n{agent_id}\n{nonce}");
    if key.verify(message.as_bytes(), &signature).is_err() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(agent_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use axum::routing::{get, post};
    use ed25519_dalek::{Signer, SigningKey};
    use tower::ServiceExt;

    fn app(pool: sqlx::PgPool) -> Router {
        Router::new()
            .route("/agent/v1/enroll", post(enroll))
            .route("/agent/v1/challenge/{agent_id}", get(challenge))
            .with_state(AppState { pool })
    }

    async fn enroll_with_token(app: &Router, token: &str, key: &SigningKey) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/agent/v1/enroll")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "token": token,
                            "public_key_hex": hex::encode(key.verifying_key().to_bytes()),
                            "hostname": "auth-test",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn nonce(app: &Router, id: Uuid) -> String {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/agent/v1/challenge/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["nonce"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn authenticate(
        pool: &sqlx::PgPool,
        id: Uuid,
        nonce: &str,
        signature: &str,
    ) -> Result<Uuid, StatusCode> {
        let mut headers = HeaderMap::new();
        headers.insert("x-hope-agent-id", id.to_string().parse().unwrap());
        headers.insert("x-hope-challenge", nonce.parse().unwrap());
        headers.insert("x-hope-signature", signature.parse().unwrap());
        authenticate_connection(pool, &headers).await
    }

    #[tokio::test]
    async fn enrollment_and_challenges_reject_expiry_tampering_replay_and_revocation() {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&database_url).await.unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        let app = app(pool.clone());
        let key = SigningKey::generate(&mut rand::rngs::OsRng);

        let expired = agents::create_enrollment_token(&pool, -1).await.unwrap();
        assert_eq!(
            enroll_with_token(&app, &expired, &key).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let token = agents::create_enrollment_token(&pool, 15).await.unwrap();
        let response = enroll_with_token(&app, &token, &key).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let id: Uuid = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["agent_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            enroll_with_token(&app, &token, &key).await.status(),
            StatusCode::UNAUTHORIZED
        );

        let challenge = nonce(&app, id).await;
        let message = format!("hope-agent-connect-v1\n{id}\n{challenge}");
        let signature = hex::encode(key.sign(message.as_bytes()).to_bytes());
        assert_eq!(
            authenticate(&pool, id, &challenge, &"00".repeat(64)).await,
            Err(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            authenticate(&pool, id, &challenge, &signature).await,
            Err(StatusCode::UNAUTHORIZED)
        );

        let accepted = nonce(&app, id).await;
        let accepted_signature = hex::encode(
            key.sign(format!("hope-agent-connect-v1\n{id}\n{accepted}").as_bytes())
                .to_bytes(),
        );
        assert_eq!(
            authenticate(&pool, id, &accepted, &accepted_signature).await,
            Ok(id)
        );
        assert_eq!(
            authenticate(&pool, id, &accepted, &accepted_signature).await,
            Err(StatusCode::UNAUTHORIZED)
        );

        let next = nonce(&app, id).await;
        let next_signature = hex::encode(
            key.sign(format!("hope-agent-connect-v1\n{id}\n{next}").as_bytes())
                .to_bytes(),
        );
        agents::revoke(&pool, id).await.unwrap();
        assert_eq!(
            authenticate(&pool, id, &next, &next_signature).await,
            Err(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/agent/v1/challenge/{id}"))
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}
