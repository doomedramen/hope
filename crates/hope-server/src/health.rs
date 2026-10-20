use crate::state::State;
use axum::{Json, extract::State as Extract, http::StatusCode};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::time::timeout;

#[derive(Clone, Default)]
pub struct BackgroundHealth(Arc<Mutex<HashMap<&'static str, Progress>>>);

#[derive(Default)]
struct Progress {
    last_success: Option<Instant>,
    last_error: Option<Instant>,
}

#[derive(Clone, Copy, Default)]
struct ProgressSnapshot {
    success_age_seconds: Option<u64>,
    error_age_seconds: Option<u64>,
    failed_after_success: bool,
}

impl BackgroundHealth {
    pub fn success(&self, name: &'static str) {
        if let Ok(mut tasks) = self.0.lock() {
            tasks.entry(name).or_default().last_success = Some(Instant::now());
        }
    }

    pub fn failure(&self, name: &'static str) {
        if let Ok(mut tasks) = self.0.lock() {
            tasks.entry(name).or_default().last_error = Some(Instant::now());
        }
    }

    fn snapshot(&self) -> HashMap<&'static str, ProgressSnapshot> {
        let now = Instant::now();
        self.0
            .lock()
            .map(|tasks| {
                tasks
                    .iter()
                    .map(|(name, progress)| {
                        let success = progress.last_success;
                        let error = progress.last_error;
                        (
                            *name,
                            ProgressSnapshot {
                                success_age_seconds: success
                                    .map(|at| now.duration_since(at).as_secs()),
                                error_age_seconds: error.map(|at| now.duration_since(at).as_secs()),
                                failed_after_success: error.is_some_and(|error| {
                                    success.is_none_or(|success| error > success)
                                }),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

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

/// Authenticated operational status. Database reads have a hard deadline and backlog queries use
/// the retention and queue indexes/bounds rather than decoding sample bodies.
pub async fn system_status(Extract(state): Extract<State>) -> (StatusCode, Json<Value>) {
    let reference = crate::state::now();
    let database_ok = matches!(
        timeout(
            Duration::from_secs(1),
            sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&state.0.db),
        )
        .await,
        Ok(Ok(1))
    );
    if !database_ok {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status":"degraded","database":"unavailable"})),
        );
    }

    let measurements = async {
        let (pending, oldest_notification): (i64, Option<f64>) =
            sqlx::query_as("SELECT COUNT(*),MIN(created_at) FROM notification_outbox")
                .fetch_one(&state.0.db)
                .await?;
        let oldest_raw_metric: Option<f64> = sqlx::query_scalar(
            "SELECT MIN(collected_at) FROM metrics WHERE resolution=30 AND collected_at<?",
        )
        .bind(reference - 86400.0)
        .fetch_one(&state.0.db)
        .await?;
        let oldest_raw_source_metric: Option<f64> = sqlx::query_scalar(
            "SELECT MIN(observed_at) FROM source_metrics WHERE resolution=30 AND observed_at<?",
        )
        .bind(reference - 86400.0)
        .fetch_one(&state.0.db)
        .await?;
        Ok::<_, sqlx::Error>((
            pending,
            oldest_notification,
            oldest_raw_metric,
            oldest_raw_source_metric,
        ))
    };
    let (pending_notifications, oldest_notification, oldest_raw_metric, oldest_raw_source_metric) =
        match timeout(Duration::from_secs(1), measurements).await {
            Ok(Ok(measurements)) => measurements,
            _ => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"status":"degraded","database":"unavailable"})),
                );
            }
        };
    let retention_lag = oldest_raw_metric.map(|at| (reference - at - 86400.0).max(0.0));
    let source_retention_lag =
        oldest_raw_source_metric.map(|at| (reference - at - 86400.0).max(0.0));
    let notification_age = oldest_notification.map(|at| (reference - at).max(0.0));

    let tasks = [
        ("discovery", 900_u64),
        ("retention", 300),
        ("notification_evaluation", 180),
        ("notification_delivery", 30),
    ];
    let progress = state.0.background.snapshot();
    let mut task_status = serde_json::Map::new();
    let mut starting = false;
    let mut degraded = false;
    for (name, stale_after_seconds) in tasks {
        let snapshot = progress.get(name).copied().unwrap_or_default();
        let status = if snapshot.failed_after_success {
            degraded = true;
            "failed"
        } else if snapshot.success_age_seconds.is_none() {
            starting = true;
            "starting"
        } else if snapshot
            .success_age_seconds
            .is_some_and(|age| age > stale_after_seconds)
        {
            degraded = true;
            "stale"
        } else {
            "ok"
        };
        task_status.insert(
            name.into(),
            json!({
                "status": status,
                "last_success_age_seconds": snapshot.success_age_seconds,
                "last_error_age_seconds": snapshot.error_age_seconds,
                "stale_after_seconds": stale_after_seconds,
            }),
        );
    }
    if notification_age.is_some_and(|age| age > 300.0)
        || retention_lag.is_some_and(|lag| lag > 3600.0)
        || source_retention_lag.is_some_and(|lag| lag > 3600.0)
    {
        degraded = true;
    }
    let status = if degraded {
        "degraded"
    } else if starting {
        "starting"
    } else {
        "healthy"
    };
    (
        StatusCode::OK,
        Json(json!({
            "status": status,
            "database": "ok",
            "background_tasks": task_status,
            "notifications": {
                "pending": pending_notifications,
                "oldest_age_seconds": notification_age,
                "degraded_after_seconds": 300,
            },
            "metrics_retention": {
                "oldest_unaggregated_at": oldest_raw_metric,
                "lag_seconds": retention_lag,
                "degraded_after_seconds": 3600,
            },
            "source_metrics_retention": {
                "oldest_unaggregated_at": oldest_raw_source_metric,
                "lag_seconds": source_retention_lag,
                "degraded_after_seconds": 3600,
            },
        })),
    )
}
