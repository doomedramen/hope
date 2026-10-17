pub mod agents;
pub mod api;
pub mod arp;
pub mod auth;
pub mod backup;
pub mod classify;
pub mod discovery;
pub mod enrichment;
pub mod error;
pub mod fingerprint;
pub mod headers;
pub mod lifecycle;
pub mod limits;
pub mod metrics;
pub mod migrations;
pub mod notify;
pub mod releases;
pub mod scanner;
pub mod ssh;
pub mod state;
pub mod topology;
pub mod validation;
pub mod vendor;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post, put},
};
use state::State;
use tower_http::services::ServeDir;

pub fn router(state: State, web_dir: &str) -> Router {
    let private = Router::new()
        .route("/bootstrap", get(api::bootstrap))
        .route("/account/password", post(auth::change_password))
        .route(
            "/notifications",
            get(notify::get_settings).put(notify::put_settings),
        )
        .route("/notifications/test", post(notify::test))
        .route("/settings/reporting", put(api::set_reporting))
        .route("/hosts/{id}/type", put(api::set_device_type))
        .route("/hosts/{id}/identity", put(api::set_host_identity))
        .route("/logout", post(auth::logout))
        .route("/networks", post(api::add_network))
        .route(
            "/networks/{id}",
            put(api::update_network).delete(api::delete_network),
        )
        .route("/networks/{id}/scan", post(api::start_scan))
        .route("/scans/{id}/cancel", post(api::cancel_scan))
        .route("/map", put(api::map).patch(api::edit_map))
        .route("/hosts/{id}/merge", post(api::merge))
        .route("/hosts/{id}/metrics", get(metrics::history))
        .route("/enrollment-tokens", post(agents::token))
        .route("/agents/{id}/revoke", post(agents::revoke))
        .route("/agents/update", post(agents::request_all_updates))
        .route("/hosts/{id}/update-agent", post(agents::request_update))
        .route("/events", get(api::events))
        .route("/ssh/fingerprint", post(ssh::fingerprint))
        .route("/ssh/install", post(ssh::install))
        .route("/ssh/jobs", get(ssh::jobs))
        .route("/ssh/jobs/{id}", get(ssh::job))
        .route("/releases", get(releases::availability))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_session,
        ));
    let public = Router::new()
        .route("/auth/status", get(auth::status))
        .route("/auth/setup", post(auth::setup))
        .route("/auth/login", post(auth::login))
        .route("/agent/enroll", post(agents::enroll))
        .route("/agent/report", post(agents::report));
    Router::new()
        .nest("/api/v1", private.merge(public))
        .route(
            "/healthz",
            get(|| async { axum::Json(serde_json::json!({"status":"ok"})) }),
        )
        .route("/agent/install", get(releases::installer))
        .nest_service(
            "/agent/releases",
            Router::new()
                .fallback(releases::serve)
                .with_state(state.clone())
                .layer(middleware::from_fn(releases::cache_headers)),
        )
        .fallback_service(ServeDir::new(web_dir).append_index_html_on_directories(true))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::check_origin,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            limits::client_key,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            headers::apply,
        ))
        .with_state(state)
}
