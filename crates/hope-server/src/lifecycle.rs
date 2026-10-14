//! Process lifecycle: graceful shutdown inside Docker's default 10 second stop grace.
use crate::state::{State, now};
use anyhow::Result;
use std::{
    future::Future,
    net::SocketAddr,
    time::{Duration, Instant},
};
use tokio::time::timeout;

/// Total time allowed between the stop signal and process exit.
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(8);

/// Resolves on SIGTERM or SIGINT (Ctrl-C).
pub async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) {
            (Ok(mut term), Ok(mut int)) => {
                tokio::select! { _ = term.recv() => {}, _ = int.recv() => {} }
            }
            _ => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Serve until `stop` resolves, then drain within `deadline`: stop scheduling, interrupt scans and
/// SSH jobs, end SSE streams, wait for connections, checkpoint and close SQLite.
pub async fn serve(
    state: State,
    listener: tokio::net::TcpListener,
    router: axum::Router,
    stop: impl Future<Output = ()> + Send + 'static,
    deadline: Duration,
) -> Result<()> {
    let signalled = std::sync::Arc::new(std::sync::OnceLock::new());
    {
        let (state, signalled) = (state.clone(), signalled.clone());
        tokio::spawn(async move {
            stop.await;
            let _ = signalled.set(Instant::now());
            tracing::info!("stop requested; shutting down");
            state.0.shutdown.cancel();
        });
    }
    let service = router.into_make_service_with_connect_info::<SocketAddr>();
    let server = axum::serve(listener, service)
        .with_graceful_shutdown(state.0.shutdown.clone().cancelled_owned());
    let drained = {
        let state = state.clone();
        async move {
            state.0.shutdown.cancelled().await;
            tokio::time::sleep(deadline.saturating_sub(Duration::from_secs(2))).await;
        }
    };
    let result = tokio::select! {
        result = server => result.map_err(anyhow::Error::from),
        _ = drained => {
            tracing::warn!("connections did not close before the deadline; closing anyway");
            Ok(())
        }
    };
    let started = signalled.get().copied().unwrap_or_else(Instant::now);
    settle(&state, deadline.saturating_sub(started.elapsed())).await;
    result
}

/// Interrupt and persist unfinished work, then close the database. Safe to call more than once.
pub async fn settle(state: &State, budget: Duration) {
    state.0.shutdown.cancel();
    for token in state.0.scans.lock().await.values() {
        token.cancel();
    }
    // SSH job tasks observe the shutdown token, abort their sessions and record `interrupted`.
    // Probe and enrichment tasks are cancelled by the scan tokens; wait until none can still
    // write, so nothing touches the database after it is closed.
    let jobs_settled = timeout(budget.min(Duration::from_secs(4)), async {
        while !state.0.installs.lock().await.is_empty()
            || state
                .0
                .active_tasks
                .load(std::sync::atomic::Ordering::SeqCst)
                > 0
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    let persist = async {
        let scans = sqlx::query("UPDATE scans SET status='interrupted', finished_at=?, error='Server stopped; start a new scan' WHERE status IN ('queued','running')")
            .bind(now())
            .execute(&state.0.db)
            .await;
        if let Err(error) = scans {
            tracing::error!(%error, "could not persist scan interruption");
        }
        // Tasks that did not settle in time must not leave their rows claiming to run.
        let jobs = sqlx::query("UPDATE install_jobs SET status='interrupted', finished_at=?, error_code='SERVER_RESTARTED', message=? WHERE status='running'")
            .bind(now())
            .bind(crate::ssh::SERVER_RESTARTED_MESSAGE)
            .execute(&state.0.db)
            .await;
        if let Err(error) = jobs {
            tracing::error!(%error, "could not persist SSH job interruption");
        }
        let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&state.0.db)
            .await;
        state.0.db.close().await;
    };
    if timeout(budget.max(Duration::from_secs(1)), persist)
        .await
        .is_err()
    {
        tracing::error!("database shutdown exceeded the deadline");
    }
    if jobs_settled.is_err() {
        tracing::warn!(
            "SSH jobs or scan tasks did not settle before the deadline; recorded as interrupted"
        );
    }
}
