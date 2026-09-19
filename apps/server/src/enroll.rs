//! Enrollment listener: TLS with server authentication only (no client
//! cert required — the agent doesn't have one yet). Exchanges a single-use
//! enrollment token + agent-generated CSR for a short-lived client
//! certificate signed by the internal CA (ADR-0007).

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::routing::post;
use axum_server::tls_rustls::RustlsConfig;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::agents;
use crate::config::Config;
use crate::pki::{self, Ca};
use crate::ratelimit::{self, RateLimitState};

#[derive(Clone)]
struct EnrollState {
    pool: PgPool,
    /// Loaded once at listener startup and reused for every request
    /// (previously reloaded from disk per request).
    ca: std::sync::Arc<Ca>,
    ca_cert_pem: std::sync::Arc<String>,
}

#[derive(Debug, Deserialize)]
struct EnrollRequest {
    token: String,
    csr_pem: String,
    hostname: Option<String>,
}

#[derive(Debug, Serialize)]
struct EnrollResponse {
    agent_id: String,
    cert_pem: String,
    ca_cert_pem: String,
}

async fn enroll(
    State(state): State<EnrollState>,
    Json(req): Json<EnrollRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    // Never log the token or any key material.
    let consumed = match agents::consume_enrollment_token(&state.pool, &req.token).await {
        Ok(v) => v,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": err.to_string() })),
            );
        }
    };

    if !consumed {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "invalid, expired, or already-used token" })),
        );
    }

    let (cert_pem, serial, fingerprint) = match pki::sign_agent_csr(&state.ca, &req.csr_pem) {
        Ok(v) => v,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("invalid CSR: {err}") })),
            );
        }
    };

    let agent_id =
        match agents::insert_agent(&state.pool, &fingerprint, &serial, req.hostname.as_deref())
            .await
        {
            Ok(id) => id,
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": err.to_string() })),
                );
            }
        };

    let response = EnrollResponse {
        agent_id: agent_id.to_string(),
        cert_pem,
        ca_cert_pem: (*state.ca_cert_pem).clone(),
    };

    (
        StatusCode::CREATED,
        Json(serde_json::to_value(response).unwrap()),
    )
}

pub async fn serve(config: Config, pool: PgPool) -> anyhow::Result<()> {
    let ca = pki::load_ca(&config)?;
    let ca_cert_pem = ca.cert.pem();
    let state = EnrollState {
        pool,
        ca: std::sync::Arc::new(ca),
        ca_cert_pem: std::sync::Arc::new(ca_cert_pem),
    };

    // Tokens are already single-use/short-lived, but this still bounds
    // how fast an attacker can burn through guesses or hammer the
    // endpoint while a valid token's TTL window is open.
    let enroll_limiter = RateLimitState::new(20, config.trust_proxy_headers);

    let app = Router::new()
        .route(
            "/enroll",
            post(enroll).layer(from_fn_with_state(enroll_limiter, ratelimit::enforce)),
        )
        .with_state(state);

    let tls_config =
        RustlsConfig::from_pem_file(&config.server_cert_path, &config.server_key_path).await?;

    let addr: std::net::SocketAddr = config.enroll_bind_addr.parse()?;
    tracing::info!(addr = %addr, "enroll listener starting");
    axum_server::bind_rustls(addr, tls_config)
        .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .await?;

    Ok(())
}
