//! Response headers applied to every response.
//!
//! The console is a Next.js static export. Its pages embed inline bootstrap scripts whose bodies
//! differ per page and build, so `script-src` allows `'unsafe-inline'`; every other source stays
//! `'self'` except for the explicitly used device and application icon CDNs; objects/framing/base-URI
//! are denied and scripts/API connections stay same-origin. React Flow and charts set inline `style` attributes, hence `style-src
//! 'unsafe-inline'`. Tightening scripts to hashes needs a build step that hashes each exported page.
use crate::state::State;
use axum::{
    extract::{Request, State as Extract},
    http::{HeaderName, HeaderValue, header},
    middleware::Next,
    response::Response,
};

pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: https://cdn.jsdelivr.net https://static.ui.com; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

pub async fn apply(Extract(state): Extract<State>, request: Request, next: Next) -> Response {
    let api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    if api {
        response = bound_rejection(response);
    }
    let headers = response.headers_mut();
    let fixed: [(HeaderName, &'static str); 5] = [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY),
        (
            HeaderName::from_static("permissions-policy"),
            "camera=(), microphone=(), geolocation=()",
        ),
    ];
    for (name, value) in fixed {
        headers.insert(name, HeaderValue::from_static(value));
    }
    // Enrollment, authentication, install-command, SSH job and every other API response may
    // carry secrets or private inventory: never store them in a browser or proxy cache.
    if api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    }
    if state.0.server_url.starts_with("https://") {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    response
}

/// Extractor rejections (malformed JSON, wrong content type, oversized body) are plain text that
/// can quote parser details or fragments of the submitted body, which may hold credentials. Replace
/// them with fixed, bounded messages; handler errors are already structured and fixed.
fn bound_rejection(response: Response) -> Response {
    use axum::{http::StatusCode, response::IntoResponse};
    let plain = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/plain"));
    let message = match response.status() {
        StatusCode::PAYLOAD_TOO_LARGE if plain => "Request body is too large",
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY if plain => {
            "Invalid request body"
        }
        StatusCode::UNSUPPORTED_MEDIA_TYPE if plain => "Content type must be application/json",
        _ => return response,
    };
    crate::error::ApiError(response.status(), message.into()).into_response()
}
