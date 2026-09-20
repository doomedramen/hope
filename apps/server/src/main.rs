mod agent_install;
mod agent_inventory;
mod agent_updates;
mod agents;
mod auth_mw;
mod config;
mod credentials;
mod csrf;
pub mod dependency_graph;
mod discovery;
mod enroll;
mod gateway;
mod inventory;
mod jobs_handlers;
pub mod maintenance;
mod monitor_checks;
mod monitor_scheduler;
mod monitoring;
mod notifications;
mod pki;
mod ratelimit;
mod release_repository;
mod routes;
mod scheduler;
mod seed;
mod session_store;
mod ssh_install;
mod ssh_trust;
mod state;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post};
use clap::{Parser, Subcommand};
use sqlx::postgres::PgPoolOptions;
use tokio::signal::unix::{SignalKind, signal};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tower_sessions::cookie::SameSite;
use tower_sessions::{Expiry, SessionManagerLayer};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use crate::config::Config;
use crate::ratelimit::RateLimitState;
use crate::session_store::PgSessionStore;
use crate::state::AppState;

#[derive(Parser)]
#[command(name = "server", about = "Homelab Operations Platform server")]
struct Cli {
    #[command(subcommand)]
    role: Role,
}

#[derive(Subcommand)]
enum Role {
    /// Run the API server (control plane, web UI) plus the agent gateway
    /// and enrollment listeners.
    Serve,
    /// Run the background worker role (job queue consumer).
    Worker,
    /// Apply pending database migrations and exit.
    Migrate,
    /// Internal CA management.
    Ca {
        #[command(subcommand)]
        action: CaAction,
    },
    /// Agent enrollment token management.
    EnrollToken {
        #[command(subcommand)]
        action: EnrollTokenAction,
    },
    /// Revoke an agent's certificate. The gateway rejects the agent's
    /// connection on its next attempt (existing open connections are not
    /// force-closed in this milestone).
    RevokeAgent { agent_id: uuid::Uuid },
    /// Seed a demo inventory topology (design docs/design/m1-inventory.md
    /// §9 slice 11) for trying out M1 without real agents/scans.
    Seed,
}

#[derive(Subcommand)]
enum CaAction {
    /// Generate a new CA keypair/cert and server leaf cert. Refuses to run
    /// if PKI files already exist at the configured paths.
    Init,
}

#[derive(Subcommand)]
enum EnrollTokenAction {
    /// Create a single-use enrollment token and print it once. The token
    /// is never logged or stored in cleartext.
    Create {
        #[arg(long, default_value_t = 15)]
        ttl_minutes: i64,
    },
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
}

async fn build_pool(config: &Config) -> anyhow::Result<sqlx::PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&config.database_url)
        .await?;
    Ok(pool)
}

async fn run_migrations(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("../../migrations").run(pool).await?;
    Ok(())
}

fn app_router(state: AppState, web_dist_dir: &str, config: &Config) -> Router {
    let session_store = PgSessionStore::new(state.pool.clone());
    let session_layer = SessionManagerLayer::new(session_store)
        .with_expiry(Expiry::OnInactivity(
            tower_sessions::cookie::time::Duration::hours(12),
        ))
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(config.cookie_secure);

    // Setup/login aren't cookie-authenticated (they establish the
    // session), so CSRF's usual "attacker rides the victim's cookie"
    // premise doesn't strictly apply to them — but the header check is
    // applied uniformly so every future mutating /api/v1 route inherits
    // it without needing to remember to add it (see csrf.rs doc comment).
    let setup_limiter = RateLimitState::new(10, config.trust_proxy_headers);
    let login_limiter = RateLimitState::new(10, config.trust_proxy_headers);

    // The generic dependency resource stays compiled for shared inventory
    // code, while M8 routes use the validated graph handlers below.
    let _ = (
        inventory::generic::dependency_edges::list,
        inventory::generic::dependency_edges::get,
        inventory::generic::dependency_edges::create,
        inventory::generic::dependency_edges::patch,
    );

    // M1 inventory: every route is session-authenticated (§12.3) and
    // subject to the same CSRF custom-header check as every other
    // mutating `/api/v1` route (see csrf.rs doc comment).
    let inventory_router = Router::new()
        .route("/api/v1/agents", get(agent_inventory::list_agents))
        .route("/api/v1/agents/{id}", get(agent_inventory::get_agent))
        .route(
            "/api/v1/agents/{id}/update",
            post(agent_updates::start_update),
        )
        .route(
            "/api/v1/agents/{id}/updates",
            get(agent_updates::list_updates),
        )
        .route(
            "/api/v1/agents/{id}/compliance",
            get(agent_updates::compliance),
        )
        .route(
            "/api/v1/agents/{id}/update-policy",
            get(agent_updates::get_policy).put(agent_updates::put_policy),
        )
        .route("/api/v1/agent-releases", get(agent_updates::list_releases))
        .route(
            "/api/v1/agent-releases/{version}",
            get(agent_updates::get_release),
        )
        .route(
            "/api/v1/agents/{id}/inventory",
            get(agent_inventory::get_agent_inventory),
        )
        .route(
            "/api/v1/agents/{id}/inventory/history",
            get(agent_inventory::get_agent_inventory_history),
        )
        .route(
            "/api/v1/agents/{id}/health",
            get(agent_inventory::get_agent_health),
        )
        .route(
            "/api/v1/agents/{id}/version",
            get(agent_inventory::get_agent_version),
        )
        .route(
            "/api/v1/networks",
            get(inventory::generic::networks::list).post(inventory::generic::networks::create),
        )
        .route(
            "/api/v1/networks/{id}",
            get(inventory::generic::networks::get)
                .patch(inventory::generic::networks::patch)
                .delete(inventory::generic::networks::delete),
        )
        .route(
            "/api/v1/networks/{id}/discovery-scope",
            post(inventory::discovery_scopes::draft),
        )
        .route(
            "/api/v1/networks/{id}/discovery-state",
            get(inventory::discovery_scopes::get_state),
        )
        .route(
            "/api/v1/networks/{id}/discovery-scope/confirm",
            post(inventory::discovery_scopes::confirm),
        )
        .route("/api/v1/networks/{id}/scans", post(discovery::runs::create))
        .route(
            "/api/v1/networks/{id}/service-collectors",
            post(discovery::collectors::create),
        )
        .route("/api/v1/scans/{id}", get(discovery::runs::get))
        .route("/api/v1/scans/{id}/cancel", post(discovery::runs::cancel))
        .route(
            "/api/v1/devices",
            get(inventory::devices::list).post(inventory::devices::create),
        )
        .route(
            "/api/v1/devices/{id}",
            get(inventory::devices::get).patch(inventory::devices::patch),
        )
        .route(
            "/api/v1/devices/{id}/merge",
            post(inventory::devices::merge),
        )
        .route(
            "/api/v1/devices/{id}/split",
            post(inventory::devices::split),
        )
        .route(
            "/api/v1/devices/{id}/undo-merge",
            post(inventory::devices::undo_merge),
        )
        .route(
            "/api/v1/devices/{id}/identity-rules",
            post(inventory::devices::pin_identifier),
        )
        .route(
            "/api/v1/devices/reconcile",
            post(inventory::identity_service::reconcile_handler),
        )
        .route(
            "/api/v1/interfaces",
            get(inventory::generic::interfaces::list).post(inventory::generic::interfaces::create),
        )
        .route(
            "/api/v1/interfaces/{id}",
            get(inventory::generic::interfaces::get).patch(inventory::generic::interfaces::patch),
        )
        .route(
            "/api/v1/addresses",
            get(inventory::addresses::list).post(inventory::addresses::assign),
        )
        .route(
            "/api/v1/workloads",
            get(inventory::generic::workloads::list).post(inventory::generic::workloads::create),
        )
        .route(
            "/api/v1/workloads/{id}",
            get(inventory::generic::workloads::get).patch(inventory::generic::workloads::patch),
        )
        .route(
            "/api/v1/services",
            get(inventory::generic::services::list).post(inventory::generic::services::create),
        )
        .route(
            "/api/v1/services/{id}",
            get(inventory::generic::services::get).patch(inventory::generic::services::patch),
        )
        .route(
            "/api/v1/endpoints",
            get(inventory::generic::endpoints::list).post(inventory::generic::endpoints::create),
        )
        .route(
            "/api/v1/endpoints/{id}",
            get(inventory::generic::endpoints::get).patch(inventory::generic::endpoints::patch),
        )
        .route(
            "/api/v1/dependencies",
            get(dependency_graph::list).post(dependency_graph::create),
        )
        .route(
            "/api/v1/dependencies/{id}",
            get(dependency_graph::get).patch(dependency_graph::patch),
        )
        .route(
            "/api/v1/dependencies/{id}/blast-radius",
            get(dependency_graph::blast_radius),
        )
        .route(
            "/api/v1/dependencies/{id}/confirm",
            post(dependency_graph::confirm),
        )
        .route(
            "/api/v1/dependencies/{id}/reject",
            post(dependency_graph::reject),
        )
        .route(
            "/api/v1/identity-suggestions",
            get(inventory::identity_service::list_suggestions),
        )
        .route(
            "/api/v1/identity-suggestions/{id}/confirm",
            post(inventory::identity_service::confirm_suggestion),
        )
        .route(
            "/api/v1/identity-suggestions/{id}/reject",
            post(inventory::identity_service::reject_suggestion),
        )
        .route(
            "/api/v1/service-reviews",
            get(inventory::service_reviews::list),
        )
        .route(
            "/api/v1/service-reviews/{id}",
            get(inventory::service_reviews::get),
        )
        .route(
            "/api/v1/service-reviews/{id}/confirm",
            post(inventory::service_reviews::confirm),
        )
        .route(
            "/api/v1/service-reviews/{id}/reject",
            post(inventory::service_reviews::reject),
        )
        .route(
            "/api/v1/monitor-proposals",
            get(inventory::monitor_proposals::list).post(inventory::monitor_proposals::generate),
        )
        .route(
            "/api/v1/monitor-proposals/generate",
            post(inventory::monitor_proposals::generate),
        )
        .route(
            "/api/v1/monitor-proposals/{id}",
            get(inventory::monitor_proposals::get).patch(inventory::monitor_proposals::patch),
        )
        .route(
            "/api/v1/monitor-proposals/{id}/approve",
            post(inventory::monitor_proposals::approve),
        )
        .route(
            "/api/v1/monitor-proposals/{id}/reject",
            post(inventory::monitor_proposals::reject),
        )
        .route(
            "/api/v1/monitors",
            get(monitoring::list).post(monitoring::create_monitor),
        )
        .route("/api/v1/monitors/{id}", get(monitoring::get))
        .route("/api/v1/monitors/{id}/results", get(monitoring::results))
        .route(
            "/api/v1/agents/{id}/monitors",
            post(monitoring::create_agent_monitor),
        )
        .route("/api/v1/incidents", get(monitoring::incidents))
        .route("/api/v1/incidents/{id}", get(monitoring::incident))
        .route(
            "/api/v1/notification-channels",
            get(notifications::list_channels).post(notifications::create_channel),
        )
        .route(
            "/api/v1/notification-routes",
            get(notifications::list_routes).post(notifications::create_route),
        )
        .route(
            "/api/v1/maintenance-events",
            get(maintenance::list).post(maintenance::create),
        )
        .route(
            "/api/v1/maintenance-events/{id}",
            get(maintenance::get).patch(maintenance::patch),
        )
        .route(
            "/api/v1/maintenance-events/{id}/start",
            post(maintenance::start),
        )
        .route(
            "/api/v1/maintenance-events/{id}/complete",
            post(maintenance::complete),
        )
        .route(
            "/api/v1/maintenance-events/{id}/cancel",
            post(maintenance::cancel),
        )
        .route(
            "/api/v1/maintenance-events/{id}/conflicts",
            get(maintenance::conflicts),
        )
        .route(
            "/api/v1/credentials",
            get(routes::credentials::list).post(routes::credentials::create),
        )
        .route(
            "/api/v1/credentials/{id}",
            get(routes::credentials::get)
                .patch(routes::credentials::update)
                .delete(routes::credentials::delete),
        )
        .route(
            "/api/v1/devices/{id}/agent-install",
            post(routes::agent_deployment::install),
        )
        .route(
            "/api/v1/devices/{id}/agent-repair",
            post(routes::agent_deployment::repair),
        )
        .route("/api/v1/jobs/{id}", get(routes::agent_deployment::get_job))
        .route("/api/v1/ssh-host-keys", get(routes::ssh_host_keys::list))
        .route(
            "/api/v1/ssh-host-keys/{id}",
            get(routes::ssh_host_keys::get),
        )
        .route(
            "/api/v1/ssh-host-keys/{id}/trust",
            post(routes::ssh_host_keys::trust),
        )
        .route(
            "/api/v1/ssh-host-keys/{id}/revoke",
            post(routes::ssh_host_keys::revoke),
        )
        .route(
            "/api/v1/services/{id}/monitor-proposals",
            post(inventory::monitor_proposals::generate_for_path),
        )
        .route("/api/v1/changes", get(inventory::changes::list))
        .route("/api/v1/audit", get(inventory::changes::list_audit))
        .route(
            "/api/v1/evidence",
            get(inventory::evidence::list).post(inventory::evidence::submit),
        )
        .route(
            "/api/v1/evidence/resolve",
            get(inventory::evidence::resolve_handler),
        )
        // Credential JSON includes private keys, but must still be bounded
        // before deserialization so oversized payloads cannot allocate
        // unbounded secret material in the API process.
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(from_fn(csrf::require_custom_header))
        .layer(from_fn(auth_mw::require_session));

    let api = Router::new()
        .route("/health/live", get(routes::health::live))
        .route("/health/ready", get(routes::health::ready))
        .route("/install-agent.sh", get(agent_install::script))
        .route(
            "/agent-download/latest/{platform}/{arch}",
            get(agent_install::latest_version),
        )
        .route(
            "/agent-download/{version}/{platform}/{arch}/{file}",
            get(agent_install::file),
        )
        .route(
            "/api/v1/setup",
            get(routes::auth::setup_status)
                .post(routes::auth::setup)
                .layer(from_fn(csrf::require_custom_header))
                .layer(from_fn_with_state(setup_limiter, ratelimit::enforce)),
        )
        .route(
            "/api/v1/login",
            post(routes::auth::login)
                .layer(from_fn(csrf::require_custom_header))
                .layer(from_fn_with_state(login_limiter, ratelimit::enforce)),
        )
        .merge(inventory_router)
        .with_state(state);

    let index = format!("{web_dist_dir}/index.html");
    let static_service = ServeDir::new(web_dist_dir).not_found_service(ServeFile::new(&index));

    api.fallback_service(static_service)
        .layer(session_layer)
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let config = Config::load()?;

    match cli.role {
        Role::Migrate => {
            let pool = build_pool(&config).await?;
            run_migrations(&pool).await?;
            tracing::info!("migrations applied");
        }
        Role::Ca { action } => match action {
            CaAction::Init => {
                pki::init(&config)?;
                tracing::info!(
                    ca_cert = %config.ca_cert_path,
                    server_cert = %config.server_cert_path,
                    "internal CA and server leaf cert generated"
                );
            }
        },
        Role::EnrollToken { action } => match action {
            EnrollTokenAction::Create { ttl_minutes } => {
                let pool = build_pool(&config).await?;
                let token = agents::create_enrollment_token(&pool, ttl_minutes).await?;
                let ca = pki::load_ca(&config)?;
                let ca_fingerprint = pki::fingerprint_der(ca.cert.der());
                // Deliberately bypasses `tracing` (JSON logs may be shipped
                // elsewhere): this is the one and only time the token is
                // available in cleartext. The CA fingerprint is not
                // secret, but is printed alongside it for convenience —
                // `agent enroll` needs both to pin the enroll TLS
                // connection (replaces TOFU).
                println!("token={token}");
                println!("ca_fingerprint_sha256={ca_fingerprint}");
                println!("code={token}.{ca_fingerprint}");
            }
        },
        Role::RevokeAgent { agent_id } => {
            let pool = build_pool(&config).await?;
            agents::revoke(&pool, agent_id).await?;
            tracing::info!(%agent_id, "agent revoked");
        }
        Role::Seed => {
            let pool = build_pool(&config).await?;
            run_migrations(&pool).await?;
            seed::run(&pool).await?;
        }
        Role::Serve => {
            let pool = build_pool(&config).await?;
            run_migrations(&pool).await?;

            let state = AppState { pool: pool.clone() };
            let app = app_router(state, &config.web_dist_dir, &config);

            let listener = tokio::net::TcpListener::bind(&config.bind_addr).await?;
            tracing::info!(addr = %config.bind_addr, "server listening");

            let gateway_config = config.clone();
            let gateway_pool = pool.clone();
            let gateway_task = tokio::spawn(async move {
                if let Err(err) = gateway::serve(gateway_config, gateway_pool).await {
                    tracing::error!(error = %err, "agent gateway stopped");
                }
            });

            let enroll_config = config.clone();
            let enroll_pool = pool.clone();
            let enroll_task = tokio::spawn(async move {
                if let Err(err) = enroll::serve(enroll_config, enroll_pool).await {
                    tracing::error!(error = %err, "enroll listener stopped");
                }
            });

            // Session cleanup and enrollment-token purge run as scheduled
            // jobs (worker role), not a bespoke timer here — see
            // scheduler.rs and jobs_handlers.rs.
            let scheduler_pool = pool.clone();
            let scheduler_task = tokio::spawn(scheduler::run(
                scheduler_pool,
                config.change_event_retention_days,
                config.monitor_result_retention_days,
                config.monitor_result_rollup_after_days,
                config.monitor_result_rollup_retention_days,
                config.audit_event_retention_days,
                config.job_retention_days,
            ));

            let api_task = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            );

            tokio::select! {
                res = api_task => { res?; }
                _ = gateway_task => {}
                _ = enroll_task => {}
                _ = scheduler_task => {}
            }
        }
        Role::Worker => {
            let pool = build_pool(&config).await?;
            // The worker can start before (or independent of) `server
            // serve` -- e.g. as its own compose service with no ordering
            // guarantee beyond "postgres is healthy" -- so it must not
            // assume migrations have already been applied.
            // `sqlx::migrate!` is safe to call concurrently from multiple
            // processes (it takes an advisory lock).
            run_migrations(&pool).await?;
            let registry = jobs_handlers::Registry::new();
            tracing::info!("worker started");

            // Cooperative shutdown: stop claiming new jobs on SIGTERM, but
            // let whatever job is already in flight finish naturally
            // (spec §17 M0's job framework should survive a restart
            // without losing/corrupting in-progress work).
            let shutdown_requested = Arc::new(AtomicBool::new(false));
            let shutdown_notify = Arc::new(tokio::sync::Notify::new());
            {
                let shutdown_requested = shutdown_requested.clone();
                let shutdown_notify = shutdown_notify.clone();
                tokio::spawn(async move {
                    let mut sigterm =
                        signal(SignalKind::terminate()).expect("install SIGTERM handler");
                    sigterm.recv().await;
                    tracing::info!("SIGTERM received: finishing any in-flight job, then stopping");
                    shutdown_requested.store(true, Ordering::SeqCst);
                    shutdown_notify.notify_waiters();
                });
            }

            // Lease reaper: a separate periodic loop (not tied to the
            // claim/dispatch cycle below) so a worker that's been busy
            // processing one long job for a while still reaps other
            // workers' abandoned leases promptly.
            {
                let pool = pool.clone();
                let shutdown_notify = shutdown_notify.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                                match jobs::reap_expired_leases(&pool).await {
                                    Ok(count) if count > 0 => {
                                        tracing::info!(count, "reaped expired job leases");
                                    }
                                    Ok(_) => {}
                                    Err(err) => {
                                        tracing::warn!(error = %err, "lease reaper failed");
                                    }
                                }
                            }
                            _ = shutdown_notify.notified() => break,
                        }
                    }
                });
            }

            let monitor_owner = format!("monitor-{}", Uuid::new_v4());
            let monitor_pool = pool.clone();
            let monitor_requested = shutdown_requested.clone();
            let monitor_shutdown = shutdown_notify.clone();
            let monitor_task = tokio::spawn(monitor_scheduler::run(
                monitor_pool,
                monitor_owner,
                monitor_requested,
                monitor_shutdown,
            ));

            loop {
                if shutdown_requested.load(Ordering::SeqCst) {
                    tracing::info!("worker stopped");
                    break;
                }

                match jobs::claim(&pool, "worker", 60).await? {
                    Some(job) => {
                        tracing::info!(job_id = %job.id, job_type = %job.job_type, "claimed job");

                        match registry.get(&job.job_type) {
                            Some(handler) => match handler
                                .handle(&pool, job.id, "worker", job.payload.clone())
                                .await
                            {
                                Ok(jobs_handlers::JobOutcome::Completed) => {
                                    jobs::complete(&pool, job.id, "worker").await?;
                                }
                                Ok(jobs_handlers::JobOutcome::Cancelled) => {
                                    jobs::cancel(&pool, job.id, "worker").await?;
                                }
                                Err(err) => {
                                    tracing::warn!(job_id = %job.id, error = %err, "job failed");
                                    jobs::fail(&pool, job.id, "worker", &err.to_string()).await?;
                                }
                            },
                            None => {
                                let message = format!("unknown job kind: {}", job.job_type);
                                tracing::error!(job_id = %job.id, %message);
                                jobs::fail_permanently(&pool, job.id, "worker", &message).await?;
                            }
                        }
                    }
                    None => {
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                            _ = shutdown_notify.notified() => {}
                        }
                    }
                }
            }
            let _ = monitor_task.await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod handshake_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
    use rustls::RootCertStore;
    use sqlx::PgPool;
    use tokio_tungstenite::Connector;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use uuid::Uuid;

    use crate::config::Config;
    use crate::{agents, enroll, gateway, pki};

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    fn test_config(work_dir: &std::path::Path, gateway_port: u16, enroll_port: u16) -> Config {
        Config {
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: std::env::var("DATABASE_URL").unwrap(),
            web_dist_dir: "apps/web/dist".to_string(),
            gateway_bind_addr: format!("127.0.0.1:{gateway_port}"),
            enroll_bind_addr: format!("127.0.0.1:{enroll_port}"),
            ca_cert_path: work_dir.join("ca-cert.pem").to_string_lossy().to_string(),
            ca_key_path: work_dir.join("ca-key.pem").to_string_lossy().to_string(),
            server_cert_path: work_dir
                .join("server-cert.pem")
                .to_string_lossy()
                .to_string(),
            server_key_path: work_dir
                .join("server-key.pem")
                .to_string_lossy()
                .to_string(),
            cookie_secure: false,
            trust_proxy_headers: false,
            change_event_retention_days: 365,
            monitor_result_retention_days: 30,
            monitor_result_rollup_after_days: 7,
            monitor_result_rollup_retention_days: 365,
            audit_event_retention_days: 365,
            job_retention_days: 30,
        }
    }

    fn generate_csr(common_name: &str) -> (KeyPair, String) {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, common_name);
        params.distinguished_name = dn;
        let csr = params.serialize_request(&key).unwrap();
        (key, csr.pem().unwrap())
    }

    struct Enrolled {
        agent_id: Uuid,
        key: KeyPair,
        cert_pem: String,
        ca_cert_pem: String,
    }

    async fn enroll_agent(
        enroll_addr: &str,
        token: &str,
        common_name: &str,
    ) -> anyhow::Result<Enrolled> {
        let (key, csr_pem) = generate_csr(common_name);

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()?;

        let response = client
            .post(format!("https://{enroll_addr}/enroll"))
            .json(&serde_json::json!({
                "token": token,
                "csr_pem": csr_pem,
                "hostname": common_name,
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            anyhow::bail!(
                "enroll failed: {} {}",
                response.status(),
                response.text().await.unwrap_or_default()
            );
        }

        let body: serde_json::Value = response.json().await?;
        Ok(Enrolled {
            agent_id: body["agent_id"].as_str().unwrap().parse()?,
            key,
            cert_pem: body["cert_pem"].as_str().unwrap().to_string(),
            ca_cert_pem: body["ca_cert_pem"].as_str().unwrap().to_string(),
        })
    }

    fn client_tls_config(enrolled: &Enrolled) -> rustls::ClientConfig {
        let certs: Vec<_> = rustls_pemfile::certs(&mut enrolled.cert_pem.as_bytes())
            .collect::<Result<_, _>>()
            .unwrap();
        let key = rustls_pemfile::private_key(&mut enrolled.key.serialize_pem().as_bytes())
            .unwrap()
            .unwrap();

        let mut roots = RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut enrolled.ca_cert_pem.as_bytes()) {
            roots.add(cert.unwrap()).unwrap();
        }

        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_client_auth_cert(certs, key)
            .unwrap()
    }

    #[tokio::test]
    async fn enroll_connect_hello_heartbeat_replay_expiry_revocation() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        let _ = rustls::crypto::ring::default_provider().install_default();

        let work_dir = std::env::temp_dir().join(format!("hope-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&work_dir).unwrap();
        let config = test_config(&work_dir, 18543, 18544);
        pki::init(&config).unwrap();

        {
            let config = config.clone();
            let pool = pool.clone();
            tokio::spawn(async move {
                let _ = enroll::serve(config, pool).await;
            });
        }
        {
            let config = config.clone();
            let pool = pool.clone();
            tokio::spawn(async move {
                let _ = gateway::serve(config, pool).await;
            });
        }
        tokio::time::sleep(Duration::from_millis(300)).await;

        // --- happy path: enroll, connect, hello/ack, heartbeat updates last_seen ---
        let token = agents::create_enrollment_token(&pool, 15).await.unwrap();
        let enrolled = enroll_agent(&config.enroll_bind_addr, &token, "test-agent-1")
            .await
            .unwrap();

        let tls_config = client_tls_config(&enrolled);
        let (ws_stream, _) = tokio_tungstenite::connect_async_tls_with_config(
            format!("wss://{}/", config.gateway_bind_addr),
            None,
            false,
            Some(Connector::Rustls(Arc::new(tls_config.clone()))),
        )
        .await
        .unwrap();
        let (mut write, mut read) = ws_stream.split();

        let hello = serde_json::json!({
            "type": "hello",
            "message_id": Uuid::new_v4(),
            "protocol_version": protocol::PROTOCOL_VERSION,
            "agent_id": enrolled.agent_id,
            "agent_version": "0.1.0",
            "hostname": "test-agent-1",
            "os": "linux",
            "arch": "x86_64",
            "capabilities": ["host", "network", "docker"]
        });
        write
            .send(WsMessage::Text(hello.to_string()))
            .await
            .unwrap();

        let ack = read.next().await.unwrap().unwrap();
        let WsMessage::Text(text) = ack else {
            panic!("expected text frame")
        };
        let envelope: protocol::Envelope = serde_json::from_str(&text).unwrap();
        assert!(matches!(
            envelope.message,
            protocol::Message::HelloAck(protocol::HelloAck { accepted: true, .. })
        ));

        let offer = protocol::Envelope::new(protocol::Message::CapabilityOffer(
            protocol::CapabilityOffer {
                supported_protocol_versions: protocol::SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
                capabilities: vec![protocol::Capability::InventorySnapshots],
            },
        ));
        write
            .send(WsMessage::Text(serde_json::to_string(&offer).unwrap()))
            .await
            .unwrap();
        let capability_ack = tokio::time::timeout(Duration::from_secs(2), read.next())
            .await
            .expect("capability negotiation should be acknowledged")
            .unwrap()
            .unwrap();
        let WsMessage::Text(capability_ack) = capability_ack else {
            panic!("expected capability acknowledgement")
        };
        let capability_ack: protocol::Envelope = serde_json::from_str(&capability_ack).unwrap();
        let protocol::Message::CapabilityAck(capability_ack) = capability_ack.message else {
            panic!("expected capability acknowledgement envelope")
        };
        assert!(capability_ack.accepted);
        assert_eq!(
            capability_ack.selected_protocol_version,
            Some(protocol::PROTOCOL_VERSION)
        );
        assert_eq!(
            capability_ack.capabilities,
            vec![protocol::Capability::InventorySnapshots]
        );

        let heartbeat =
            protocol::Envelope::new(protocol::Message::Heartbeat(protocol::Heartbeat {
                agent_id: enrolled.agent_id,
                uptime_secs: 1,
            }));
        write
            .send(WsMessage::Text(serde_json::to_string(&heartbeat).unwrap()))
            .await
            .unwrap();
        let ack = read.next().await.unwrap().unwrap();
        assert!(matches!(ack, WsMessage::Text(_)));

        let snapshot = serde_json::json!({
            "type": "inventory_snapshot",
            "message_id": Uuid::new_v4(),
            "protocol_version": protocol::PROTOCOL_VERSION,
            "agent_id": enrolled.agent_id,
            "sequence": 1,
            "collected_at_unix_secs": time::OffsetDateTime::now_utc().unix_timestamp(),
            "complete": true,
            "inventory": {
                "hostname": "test-agent-1",
                "machine_id": format!("machine-{}", enrolled.agent_id),
                "hardware_uuid": format!("hardware-{}", enrolled.agent_id),
                "interfaces": [{
                    "name": "eth0",
                    "mac": "aa:bb:cc:dd:ee:11",
                    "addresses": [{"ip": "192.0.2.11", "type": "dhcp"}]
                }],
                "containers": [{
                    "id": format!("container-{}", enrolled.agent_id),
                    "name": "web",
                    "image": "nginx:latest",
                    "status": "running",
                    "published_ports": [{"host_ip": "192.0.2.11", "host_port": 8080, "protocol": "tcp"}]
                }],
                "sockets": [{"address": "127.0.0.1", "port": 22, "protocol": "tcp"}]
            }
        });
        write
            .send(WsMessage::Text(snapshot.to_string()))
            .await
            .unwrap();
        let inventory_ack = read.next().await.unwrap().unwrap();
        let WsMessage::Text(inventory_ack) = inventory_ack else {
            panic!("expected inventory acknowledgement")
        };
        let inventory_ack: serde_json::Value = serde_json::from_str(&inventory_ack).unwrap();
        assert_eq!(inventory_ack["type"], "inventory_snapshot_ack");
        assert_eq!(inventory_ack["accepted"], true);

        let record = agents::find_by_fingerprint(
            &pool,
            &pki::fingerprint_der(
                rustls_pemfile::certs(&mut enrolled.cert_pem.as_bytes())
                    .next()
                    .unwrap()
                    .unwrap()
                    .as_ref(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(record.id, enrolled.agent_id);

        let last_seen: Option<time::OffsetDateTime> =
            sqlx::query_scalar("select last_seen from agents where id = $1")
                .bind(enrolled.agent_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            last_seen.is_some(),
            "last_seen should be set after heartbeat"
        );
        let capabilities: serde_json::Value =
            sqlx::query_scalar("select capabilities from agents where id = $1")
                .bind(enrolled.agent_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            capabilities,
            serde_json::json!(["host", "network", "docker"])
        );
        let current_inventory: i64 =
            sqlx::query_scalar("select count(*) from agent_inventory_current where agent_id = $1")
                .bind(enrolled.agent_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(current_inventory, 1);

        drop(write);

        // --- replayed token is rejected ---
        let replay = enroll_agent(&config.enroll_bind_addr, &token, "test-agent-replay").await;
        assert!(replay.is_err(), "replayed token must be rejected");

        // --- expired token is rejected ---
        let expired_token = agents::create_enrollment_token(&pool, -1).await.unwrap();
        let expired = enroll_agent(
            &config.enroll_bind_addr,
            &expired_token,
            "test-agent-expired",
        )
        .await;
        assert!(expired.is_err(), "expired token must be rejected");

        // --- revoked agent is rejected at the gateway ---
        agents::revoke(&pool, enrolled.agent_id).await.unwrap();

        // The server closes the raw connection as soon as it sees the
        // revoked cert, before completing the WebSocket upgrade — so the
        // connect attempt itself is the thing that's expected to fail.
        let connect_result = tokio_tungstenite::connect_async_tls_with_config(
            format!("wss://{}/", config.gateway_bind_addr),
            None,
            false,
            Some(Connector::Rustls(Arc::new(tls_config))),
        )
        .await;

        match connect_result {
            Err(_) => {}
            Ok((ws_stream2, _)) => {
                let (mut write2, mut read2) = ws_stream2.split();
                let hello2 = protocol::Envelope::new(protocol::Message::Hello(protocol::Hello {
                    agent_id: Uuid::new_v4(),
                    agent_version: "0.1.0".into(),
                    hostname: "test-agent-1".into(),
                    os: "linux".into(),
                    arch: "x86_64".into(),
                }));
                let _ = write2
                    .send(WsMessage::Text(serde_json::to_string(&hello2).unwrap()))
                    .await;
                let next = tokio::time::timeout(Duration::from_secs(5), read2.next()).await;
                match next {
                    Ok(Some(Ok(WsMessage::Close(_)))) | Ok(None) | Err(_) => {}
                    Ok(Some(Ok(other))) => {
                        panic!("expected close/error for revoked agent, got {other:?}")
                    }
                    Ok(Some(Err(_))) => {}
                }
            }
        }

        std::fs::remove_dir_all(&work_dir).ok();
    }
}
