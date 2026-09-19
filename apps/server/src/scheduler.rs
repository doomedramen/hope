//! Periodic maintenance jobs and discovery change scans, enqueued (not run
//! directly) so the worker's handler registry, retry/backoff, and lease
//! semantics apply uniformly.
//!
//! Each period gets its own idempotency key, so re-running the scheduler
//! loop within the same period is a no-op (`jobs::enqueue` upserts on
//! `(job_type, idempotency_key)` without resetting status) rather than
//! creating duplicate work.

use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);
const DISCOVERY_JOB_TYPE: &str = "discovery.full_tcp";
const PORTS_PER_TARGET: i64 = 65_535;

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

pub(crate) async fn enqueue_periodic_jobs(
    pool: &PgPool,
    change_event_retention_days: i64,
    monitor_result_retention_days: i64,
    monitor_result_rollup_after_days: i64,
) {
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
        "monitor_results.retention",
        &retention_key,
        serde_json::json!({
            "retention_days": monitor_result_retention_days,
            "rollup_after_days": monitor_result_rollup_after_days,
        }),
    )
    .await
    {
        tracing::warn!(error = %err, "failed to enqueue monitor_results.retention");
    }

    let change_event_retention_key = daily_period_key("daily-change-events");
    if let Err(err) = jobs::enqueue(
        pool,
        "change_events.retention",
        &change_event_retention_key,
        serde_json::json!({"retention_days": change_event_retention_days}),
    )
    .await
    {
        tracing::warn!(error = %err, "failed to enqueue change_events.retention");
    }

    match enqueue_due_change_scans(pool).await {
        Ok(enqueued) if enqueued > 0 => {
            tracing::info!(count = enqueued, "enqueued discovery change scans");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "failed to plan discovery change scans"),
    }
}

/// Enqueue one change scan for each scope whose full-TCP cadence is due.
///
/// The planner derives all state from PostgreSQL, so a server restart does not
/// lose a due scan. Each scope transaction re-checks the safety gate while
/// locking the scope row, then creates the job and scan run together. The job
/// idempotency key includes the scope and cadence window, which also makes
/// concurrent scheduler instances safe.
pub(crate) async fn enqueue_due_change_scans(pool: &PgPool) -> anyhow::Result<usize> {
    let candidates: Vec<(Uuid,)> = sqlx::query_as(
        "select network_id from discovery_scopes \
         where enabled and confirmed_at is not null \
           and confirmed_target_count = target_count",
    )
    .fetch_all(pool)
    .await?;

    let mut enqueued = 0;
    for (network_id,) in candidates {
        match enqueue_change_scan_for_scope(pool, network_id).await {
            Ok(true) => enqueued += 1,
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%network_id, error = %error, "failed to enqueue discovery change scan");
            }
        }
    }

    Ok(enqueued)
}

/// Re-check scope safety and due state inside the transaction that creates the
/// job/run pair. M2 has no persisted quiet-period or maintenance hook yet;
/// there is no bypass here for an unconfirmed or disabled scope.
async fn enqueue_change_scan_for_scope(pool: &PgPool, network_id: Uuid) -> anyhow::Result<bool> {
    let mut transaction = pool.begin().await?;
    let scope: Option<(i64, i32, i32, i64)> = sqlx::query_as(
        "select target_count, version, full_tcp_interval_seconds, \
                floor(extract(epoch from now()) / full_tcp_interval_seconds::double precision)::bigint \
         from discovery_scopes \
         where network_id = $1 \
           and enabled \
           and confirmed_at is not null \
           and confirmed_target_count = target_count \
           and not exists ( \
               select 1 from scan_runs \
               where scan_runs.network_id = discovery_scopes.network_id \
                 and ( \
                     scan_runs.status in ('pending', 'running') \
                     or scan_runs.created_at > now() - make_interval( \
                         secs => discovery_scopes.full_tcp_interval_seconds::double precision \
                     ) \
                 ) \
           ) \
         for update",
    )
    .bind(network_id)
    .fetch_optional(&mut *transaction)
    .await?;

    let Some((target_count, scope_version, _interval_seconds, cadence_window)) = scope else {
        return Ok(false);
    };
    let ports_planned = target_count
        .checked_mul(PORTS_PER_TARGET)
        .ok_or_else(|| anyhow::anyhow!("discovery scope is too large to scan"))?;
    let idempotency_key = format!("discovery.change_scan:{network_id}:{cadence_window}");
    let job_id = jobs::enqueue_in(
        &mut transaction,
        DISCOVERY_JOB_TYPE,
        &idempotency_key,
        serde_json::json!({
            "network_id": network_id,
            "kind": "change_scan",
            "scope_version": scope_version,
        }),
    )
    .await?;

    let created: Option<(Uuid,)> = sqlx::query_as(
        "insert into scan_runs \
             (network_id, job_id, kind, scope_version, targets_planned, ports_planned, source) \
         values ($1, $2, 'change_scan', $3, $4, $5, 'scheduler') \
         on conflict (job_id) do nothing \
         returning id",
    )
    .bind(network_id)
    .bind(job_id)
    .bind(scope_version)
    .bind(target_count)
    .bind(ports_planned)
    .fetch_optional(&mut *transaction)
    .await?;

    if created.is_none() {
        let run_exists: Option<(Uuid,)> =
            sqlx::query_as("select id from scan_runs where job_id = $1")
                .bind(job_id)
                .fetch_optional(&mut *transaction)
                .await?;
        if run_exists.is_none() {
            anyhow::bail!("job {job_id} exists without its scan run");
        }
    }

    transaction.commit().await?;
    Ok(created.is_some())
}

/// Runs forever, enqueuing this period's maintenance jobs every
/// `CHECK_INTERVAL`. Enqueuing immediately on startup means a
/// short-lived/restarted server doesn't wait a full interval before the
/// first check-and-enqueue.
pub async fn run(
    pool: PgPool,
    change_event_retention_days: i64,
    monitor_result_retention_days: i64,
    monitor_result_rollup_after_days: i64,
) {
    loop {
        enqueue_periodic_jobs(
            &pool,
            change_event_retention_days,
            monitor_result_retention_days,
            monitor_result_rollup_after_days,
        )
        .await;
        tokio::time::sleep(CHECK_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    static TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

    async fn test_lock() -> tokio::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await
    }

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
        let _lock = test_lock().await;
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        enqueue_periodic_jobs(&pool, 365, 90, 7).await;
        let after_first: (i64,) =
            sqlx::query_as("select count(*) from jobs where job_type = 'session.cleanup'")
                .fetch_one(&pool)
                .await
                .unwrap();

        enqueue_periodic_jobs(&pool, 365, 90, 7).await;
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
        let _lock = test_lock().await;
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        enqueue_periodic_jobs(&pool, 365, 90, 7).await;

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
        let monitor_retention: (i64,) = sqlx::query_as(
            "select count(*) from jobs where job_type = 'monitor_results.retention'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert!(session_cleanup.0 >= 1);
        assert!(token_purge.0 >= 1);
        assert!(monitor_retention.0 >= 1);
    }

    async fn create_test_scope(pool: &PgPool, enabled: bool, confirmed: bool) -> Uuid {
        let network_id: (Uuid,) = sqlx::query_as(
            "insert into networks (cidr, name) values ('192.168.250.0/30', $1) returning id",
        )
        .bind(format!("scheduler-test-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .unwrap();

        sqlx::query(
            "insert into discovery_scopes \
                (network_id, target_count, confirmed_target_count, confirmed_at, enabled, full_tcp_interval_seconds) \
             values ($1, 2, $2, case when $3 then now() else null end, $4, 3600)",
        )
        .bind(network_id.0)
        .bind(confirmed.then_some(2_i64))
        .bind(confirmed)
        .bind(enabled)
        .execute(pool)
        .await
        .unwrap();

        network_id.0
    }

    #[tokio::test]
    async fn change_scan_planner_excludes_unconfirmed_and_disabled_scopes() {
        let _lock = test_lock().await;
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        let eligible = create_test_scope(&pool, true, true).await;
        let unconfirmed = create_test_scope(&pool, true, false).await;
        let disabled = create_test_scope(&pool, false, true).await;

        enqueue_due_change_scans(&pool).await.unwrap();

        for (network_id, expected) in [(eligible, 1_i64), (unconfirmed, 0), (disabled, 0)] {
            let count: (i64,) = sqlx::query_as(
                "select count(*) from scan_runs \
                 where network_id = $1 and kind = 'change_scan' and source = 'scheduler'",
            )
            .bind(network_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                count.0, expected,
                "unexpected scheduled runs for {network_id}"
            );
        }
    }

    #[tokio::test]
    async fn change_scan_planner_is_idempotent_within_cadence_window() {
        let _lock = test_lock().await;
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        let network_id = create_test_scope(&pool, true, true).await;

        let first = enqueue_due_change_scans(&pool).await.unwrap();
        let second = enqueue_due_change_scans(&pool).await.unwrap();

        assert_eq!(first, 1);
        assert_eq!(second, 0);

        let jobs: (i64,) = sqlx::query_as(
            "select count(*) from jobs \
             where job_type = $1 and payload ->> 'network_id' = $2 and payload ->> 'kind' = 'change_scan'",
        )
        .bind(DISCOVERY_JOB_TYPE)
        .bind(network_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(jobs.0, 1);

        let runs: (i64,) = sqlx::query_as(
            "select count(*) from scan_runs \
             where network_id = $1 and kind = 'change_scan' and source = 'scheduler'",
        )
        .bind(network_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(runs.0, 1);
    }
}
