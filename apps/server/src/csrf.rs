//! CSRF defense (spec §12.3). No cookie-authenticated mutating route
//! exists yet as of this milestone (`/api/v1/setup` and `/api/v1/login`
//! are pre-session — they're what *establishes* a session, not routes
//! that use one), but the mechanism is wired in now so every future
//! mutating `/api/v1/*` route gets it automatically rather than needing
//! each new route to remember to add it.
//!
//! Two layers, per spec's "secure session cookies, CSRF protection":
//! 1. The session cookie itself is `SameSite=Lax` (see `main.rs`), which
//!    stops a cross-site `<form>` POST from ever carrying the cookie in
//!    the first place for most real-world CSRF vectors.
//! 2. Defense in depth: this middleware requires a custom header on every
//!    unsafe-method request. A plain cross-site form/`<img>`/link-based
//!    CSRF attack cannot attach custom headers, and a `fetch`/XHR from
//!    another origin that tried to would trigger a CORS preflight the
//!    server doesn't need to (and currently doesn't) allow.

use axum::Json;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

const HEADER_NAME: &str = "x-requested-with";
const HEADER_VALUE: &str = "hope";

pub async fn require_custom_header(request: Request, next: Next) -> Response {
    let is_unsafe = matches!(
        *request.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );

    if is_unsafe {
        let has_header = request
            .headers()
            .get(HEADER_NAME)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case(HEADER_VALUE));

        if !has_header {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": format!(
                        "missing or invalid {HEADER_NAME} header (CSRF protection)"
                    )
                })),
            )
                .into_response();
        }
    }

    next.run(request).await
}
