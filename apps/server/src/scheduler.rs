//! Periodic maintenance jobs, enqueued (not run directly) so the worker's
//! handler registry, retry/backoff, and lease semantics apply uniformly.
//!
//! Each period gets its own idempotency key, so re-running the scheduler
//! loop within the same period is a no-op (`jobs::enqueue` upserts on
//! `(job_type, idempotency_key)` without resetting status) rather than
//! creating duplicate work.

use sqlx::PgPool;
use std::time::Duration;

const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Which hour-long period `now` falls in, as a stable string suitable for
/// use as an idempotency key. Two calls within the same hour produce the
/// same key.
fn hourly_period_key(prefix: &str) -> String {
    let hour_bucket = jiff::Timestamp::now().as_second() / 3600;
    format!("{prefix}-{hour_bucket}")
}

/// Which day-long period `now` falls in -- `change_events.retention`
/// doesn't need to run more than once a day.
fn daily_period_key(prefix: &str) -> String {
    let day_bucket = jiff::Timestamp::now().as_second() / 86_400;
    format!("{prefix}-{day_bucket}")
}

pub(crate) async fn enqueue_periodic_jobs(pool: &PgPool, change_event_retention_days: i64) {
    let session_cleanup_key = hourly_period_key("hourly");
    if let Err(err) = jobs::enqueue(
        pool,
        "session.cleanup",
        &session_cleanup_key,
        serde_json::json!({}),
    )
    .await
    {
        tracing::warn!(error = %err, "failed to enqueue session.cleanup");
    }

    let token_purge_key = hourly_period_key("hourly");
    if let Err(err) = jobs::enqueue(
        pool,
        "enrollment_token.purge",
        &token_purge_key,
        serde_json::json!({}),
    )
    .await
    {
        tracing::warn!(error = %err, "failed to enqueue enrollment_token.purge");
    }

    let retention_key = daily_period_key("daily");
    if let Err(err) = jobs::enqueue(
        pool,
        "change_events.retention",
        &retention_key,
        serde_json::json!({"retention_days": change_event_retention_days}),
    )
    .await
    {
        tracing::warn!(error = %err, "failed to enqueue change_events.retention");
    }
}

/// Runs forever, enqueuing this period's maintenance jobs every
/// `CHECK_INTERVAL`. Enqueuing immediately on startup means a
/// short-lived/restarted server doesn't wait a full interval before the
/// first check-and-enqueue.
pub async fn run(pool: PgPool, change_event_retention_days: i64) {
    loop {
        enqueue_periodic_jobs(&pool, change_event_retention_days).await;
        tokio::time::sleep(CHECK_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    #[tokio::test]
    async fn enqueuing_twice_in_the_same_period_does_not_duplicate() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        enqueue_periodic_jobs(&pool, 365).await;
        let after_first: (i64,) =
            sqlx::query_as("select count(*) from jobs where job_type = 'session.cleanup'")
                .fetch_one(&pool)
                .await
                .unwrap();

        enqueue_periodic_jobs(&pool, 365).await;
        let after_second: (i64,) =
            sqlx::query_as("select count(*) from jobs where job_type = 'session.cleanup'")
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(after_first.0, after_second.0);
        assert!(after_first.0 >= 1);
    }

    #[tokio::test]
    async fn enqueues_both_job_kinds() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        enqueue_periodic_jobs(&pool, 365).await;

        let session_cleanup: (i64,) =
            sqlx::query_as("select count(*) from jobs where job_type = 'session.cleanup'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let token_purge: (i64,) =
            sqlx::query_as("select count(*) from jobs where job_type = 'enrollment_token.purge'")
                .fetch_one(&pool)
                .await
                .unwrap();

        assert!(session_cleanup.0 >= 1);
        assert!(token_purge.0 >= 1);
    }
}
