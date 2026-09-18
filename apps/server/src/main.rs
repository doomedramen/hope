mod config;
mod routes;
mod state;

use std::time::Duration;

use axum::Router;
use axum::routing::{get, post};
use clap::{Parser, Subcommand};
use sqlx::postgres::PgPoolOptions;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tower_sessions::{Expiry, MemoryStore, SessionManagerLayer};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::state::AppState;

#[derive(Parser)]
#[command(name = "server", about = "Homelab Operations Platform server")]
struct Cli {
    #[command(subcommand)]
    role: Role,
}

#[derive(Subcommand)]
enum Role {
    /// Run the API server (control plane, web UI, agent gateway).
    Serve,
    /// Run the background worker role (job queue consumer).
    Worker,
    /// Apply pending database migrations and exit.
    Migrate,
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

fn app_router(state: AppState, web_dist_dir: &str) -> Router {
    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store).with_expiry(Expiry::OnInactivity(
        tower_sessions::cookie::time::Duration::hours(12),
    ));

    let api = Router::new()
        .route("/health/live", get(routes::health::live))
        .route("/health/ready", get(routes::health::ready))
        .route("/api/v1/setup", post(routes::auth::setup))
        .route("/api/v1/login", post(routes::auth::login))
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
        Role::Serve => {
            let pool = build_pool(&config).await?;
            run_migrations(&pool).await?;

            let state = AppState { pool };
            let app = app_router(state, &config.web_dist_dir);

            let listener = tokio::net::TcpListener::bind(&config.bind_addr).await?;
            tracing::info!(addr = %config.bind_addr, "server listening");
            axum::serve(listener, app).await?;
        }
        Role::Worker => {
            let pool = build_pool(&config).await?;
            tracing::info!("worker started");

            loop {
                let reaped = jobs::reap_expired_leases(&pool).await?;
                if reaped > 0 {
                    tracing::info!(count = reaped, "reaped expired job leases");
                }

                match jobs::claim(&pool, "worker", 60).await? {
                    Some(job) => {
                        tracing::info!(job_id = %job.id, job_type = %job.job_type, "claimed job");
                        // Milestone 0: no job handlers registered yet; mark
                        // claimed jobs complete so the queue plumbing is
                        // exercised end-to-end without doing real work.
                        jobs::complete(&pool, job.id, "worker").await?;
                    }
                    None => tokio::time::sleep(Duration::from_secs(2)).await,
                }
            }
        }
    }

    Ok(())
}
