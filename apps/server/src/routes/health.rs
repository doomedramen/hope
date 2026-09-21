use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;

use crate::release_repository::ReleaseRepository;
use crate::state::AppState;

/// Liveness: the process is up and can respond. Never checks dependencies.
pub async fn live() -> Json<serde_json::Value> {
    Json(json!({ "status": "live" }))
}

/// Readiness: the process can serve real traffic, i.e. the database is
/// reachable.
pub async fn ready(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    if std::env::var("HOPE_REQUIRE_BUNDLED_AGENTS").as_deref() == Ok("true") {
        let verified = ReleaseRepository::from_environment().and_then(|repository| {
            repository.latest_compatible_for_channel(
                "linux",
                "amd64",
                protocol::PROTOCOL_VERSION,
                release::ReleaseChannel::Stable,
            )?;
            Ok(())
        });
        if verified.is_err() {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(
                    json!({ "status": "not_ready", "error": "signed agent bundle is unavailable" }),
                ),
            );
        }
    }
    match sqlx::query("select 1").execute(&state.pool).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "status": "ready" }))),
        Err(err) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "not_ready", "error": err.to_string() })),
        ),
    }
}
