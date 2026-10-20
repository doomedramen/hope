use anyhow::{Context, Result};
use hope_server::{
    metrics, scanner,
    state::{State, hash, secret},
};

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("export-types") {
        print!("{}", hope_protocol::typescript());
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("backup") {
        let destination = std::env::args()
            .nth(2)
            .context("usage: hope-server backup DESTINATION.db")?;
        let data = std::env::var("HOPE_DATA_DIR").unwrap_or_else(|_| "data".into());
        hope_server::backup::run(
            &std::path::Path::new(&data).join("hope.db"),
            std::path::Path::new(&destination),
        )
        .await?;
        println!("Backup written to {destination}");
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hope_server=info".into()),
        )
        .init();
    let data = std::env::var("HOPE_DATA_DIR").unwrap_or_else(|_| "data".into());
    tokio::fs::create_dir_all(&data).await?;
    let server_url = std::env::var("HOPE_SERVER_URL")
        .unwrap_or_else(|_| "http://localhost:8080".into())
        .trim_end_matches('/')
        .to_string();
    let url = url::Url::parse(&server_url).context("HOPE_SERVER_URL must be an absolute URL")?;
    anyhow::ensure!(
        url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none(),
        "HOPE_SERVER_URL must contain only scheme and authority"
    );
    anyhow::ensure!(
        url.scheme() == "https"
            || (url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))),
        "Use HTTPS, except for localhost development"
    );
    let state = State::open(
        &format!("{data}/hope.db"),
        server_url,
        std::env::var("HOPE_RELEASE_DIR").unwrap_or_else(|_| "releases".into()),
        scanner::capabilities(),
    )
    .await?;
    if state.meta("password_hash").await?.is_none() {
        let token = std::env::var("HOPE_SETUP_TOKEN").unwrap_or_else(|_| secret());
        anyhow::ensure!(
            token.len() >= 24,
            "HOPE_SETUP_TOKEN must contain at least 24 characters"
        );
        state.set_meta("setup_token_hash", &hash(&token)).await?;
        eprintln!("H.O.P.E first-run setup token: {token}");
    }
    let maintenance = state.clone();
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = maintenance.0.shutdown.cancelled() => break,
                _ = timer.tick() => {}
            }
            if let Err(error) = metrics::retain(&maintenance).await {
                maintenance.0.background.failure("retention");
                tracing::error!(%error,"metric retention failed");
            } else {
                maintenance.0.background.success("retention");
            }
        }
    });
    hope_server::discovery::spawn(state.clone());
    hope_server::notify::spawn(state.clone());
    let bind = std::env::var("HOPE_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind,"H.O.P.E is listening");
    let router = hope_server::router(
        state.clone(),
        &std::env::var("HOPE_WEB_DIR").unwrap_or_else(|_| "web/out".into()),
    );
    hope_server::lifecycle::serve(
        state,
        listener,
        router,
        hope_server::lifecycle::stop_signal(),
        hope_server::lifecycle::SHUTDOWN_DEADLINE,
    )
    .await
}
