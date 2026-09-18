use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;

use crate::state::AppState;

/// Liveness: the process is up and can respond. Never checks dependencies.
pub async fn live() -> Json<serde_json::Value> {
    Json(json!({ "status": "live" }))
}

/// Readiness: the process can serve real traffic, i.e. the database is
/// reachable.
pub async fn ready(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    match sqlx::query("select 1").execute(&state.pool).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "status": "ready" }))),
        Err(err) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "not_ready", "error": err.to_string() })),
        ),
    }
}
