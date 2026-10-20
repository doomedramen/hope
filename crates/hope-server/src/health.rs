use crate::state::State;
use axum::{Json, extract::State as Extract, http::StatusCode};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::timeout;

/// Bounded readiness check for the state store and process shutdown lifecycle.
pub async fn readiness(Extract(state): Extract<State>) -> (StatusCode, Json<Value>) {
    let stopping = state.0.shutdown.is_cancelled();
    let database = if stopping {
        "not_checked"
    } else {
        match timeout(
            Duration::from_secs(1),
            sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&state.0.db),
        )
        .await
        {
            Ok(Ok(1)) => "ok",
            _ => "unavailable",
        }
    };
    let ready = !stopping && database == "ok";
    let response = json!({
        "status": if ready { "ready" } else { "not_ready" },
        "checks": {
            "database": database,
            "shutdown": if stopping { "stopping" } else { "running" },
        },
    });
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(response),
    )
}
