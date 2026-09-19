//! Session-auth middleware for `/api/v1` routes beyond setup/login (spec
//! §12.3). Rejects with 401 when no `user_id` is present in the session;
//! on success, inserts the parsed `Uuid` as a request extension so
//! handlers/audit logging can read it without re-touching the session.

use axum::Json;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use tower_sessions::Session;
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub struct CurrentUser(pub Uuid);

pub async fn require_session(session: Session, mut request: Request, next: Next) -> Response {
    let user_id: Option<String> = session.get("user_id").await.unwrap_or(None);
    let Some(user_id) = user_id.and_then(|s| Uuid::parse_str(&s).ok()) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "authentication required" })),
        )
            .into_response();
    };

    request.extensions_mut().insert(CurrentUser(user_id));
    next.run(request).await
}
