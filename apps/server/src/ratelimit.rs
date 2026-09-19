//! Per-IP rate limiting for unauthenticated, abuse-prone endpoints (spec
//! §12.3: "secure session cookies, CSRF protection, rate limiting").
//!
//! Applied per-route via `axum::middleware::from_fn_with_state`, not
//! globally, so each endpoint gets its own bucket and quota. Uses the
//! `governor` crate's keyed rate limiter (a GCRA token bucket per key) —
//! not `tower_governor`, since that crate's own axum/IP-extraction glue
//! adds a dependency surface we don't need for two routes.

use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU32;
use std::sync::Arc;

use axum::Json;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use governor::clock::DefaultClock;
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};
use serde_json::json;

type Limiter = RateLimiter<IpAddr, DefaultKeyedStateStore<IpAddr>, DefaultClock>;

#[derive(Clone)]
pub struct RateLimitState {
    limiter: Arc<Limiter>,
    trust_proxy_headers: bool,
}

impl RateLimitState {
    /// `per_minute` requests are allowed per client IP, per minute, with
    /// burst equal to that same figure (a simple, generous-enough default
    /// for a homelab-scale deployment).
    pub fn new(per_minute: u32, trust_proxy_headers: bool) -> Self {
        let quota = Quota::per_minute(NonZeroU32::new(per_minute.max(1)).unwrap());
        Self {
            limiter: Arc::new(RateLimiter::keyed(quota)),
            trust_proxy_headers,
        }
    }
}

/// Determine the client IP for rate-limiting purposes. Defaults to the
/// TCP peer address; only trusts `X-Forwarded-For` when explicitly
/// configured to sit behind a reverse proxy (otherwise a client could
/// spoof the header to evade or attribute its own rate limit to someone
/// else).
fn client_ip(headers: &HeaderMap, peer: SocketAddr, trust_proxy_headers: bool) -> IpAddr {
    if trust_proxy_headers
        && let Some(value) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
        && let Some(first) = value.split(',').next()
        && let Ok(ip) = first.trim().parse::<IpAddr>()
    {
        return ip;
    }
    peer.ip()
}

pub async fn enforce(
    State(state): State<RateLimitState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let ip = client_ip(request.headers(), peer, state.trust_proxy_headers);

    match state.limiter.check_key(&ip) {
        Ok(_) => next.run(request).await,
        Err(_) => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "rate limit exceeded, try again later" })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    use tower::ServiceExt;

    async fn ok_handler() -> &'static str {
        "ok"
    }

    fn test_router(per_minute: u32) -> Router {
        let state = RateLimitState::new(per_minute, false);
        Router::new()
            .route("/", get(ok_handler))
            .layer(from_fn_with_state(state, enforce))
    }

    fn request_from(peer: SocketAddr) -> HttpRequest<Body> {
        let mut request = HttpRequest::builder().uri("/").body(Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(peer));
        request
    }

    #[tokio::test]
    async fn allows_requests_within_quota_then_429s() {
        let app = test_router(2);
        let peer: SocketAddr = "127.0.0.1:9999".parse().unwrap();

        let first = app.clone().oneshot(request_from(peer)).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let second = app.clone().oneshot(request_from(peer)).await.unwrap();
        assert_eq!(second.status(), StatusCode::OK);

        let third = app.clone().oneshot(request_from(peer)).await.unwrap();
        assert_eq!(third.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn different_ips_have_independent_buckets() {
        let app = test_router(1);
        // Different IPs, not just different ports: the bucket key is the
        // IP alone (see `client_ip`), matching real-world rate limiting
        // (a client's port changes per connection; its IP doesn't).
        let peer_a: SocketAddr = "127.0.0.1:1111".parse().unwrap();
        let peer_b: SocketAddr = "127.0.0.2:1111".parse().unwrap();

        let a1 = app.clone().oneshot(request_from(peer_a)).await.unwrap();
        assert_eq!(a1.status(), StatusCode::OK);

        // Second request from the SAME ip is rejected...
        let a2 = app.clone().oneshot(request_from(peer_a)).await.unwrap();
        assert_eq!(a2.status(), StatusCode::TOO_MANY_REQUESTS);

        // ...but a different IP still has its own untouched bucket.
        let b1 = app.clone().oneshot(request_from(peer_b)).await.unwrap();
        assert_eq!(b1.status(), StatusCode::OK);
    }

    #[test]
    fn trusts_forwarded_header_only_when_configured() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.7, 10.0.0.1".parse().unwrap());
        let peer: SocketAddr = "127.0.0.1:5555".parse().unwrap();

        assert_eq!(client_ip(&headers, peer, false), peer.ip());
        assert_eq!(
            client_ip(&headers, peer, true),
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
    }
}
