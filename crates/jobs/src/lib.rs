//! Custom PostgreSQL-backed job queue (ADR-0006): `FOR UPDATE SKIP LOCKED`
//! dequeue, lease + heartbeat for crash recovery, exponential backoff for
//! retries, and a unique idempotency key per logical job.
//!
//! Deliberately uses runtime-checked `sqlx::query` (not the `query!` macro)
//! so the workspace builds without a live database connection at compile
//! time.
//!
//! `claim()` picks the oldest pending job across the *entire* table (real
//! multi-worker queues need that), so DB-gated tests across this and
//! other crates/modules are not isolated from each other when run in
//! parallel against the same database — one test's `claim()` can grab
//! another concurrently-running test's job. Run DB-gated tests with
//! `cargo test -- --test-threads=1` (see `justfile`/CI); this was found
//! and fixed after two jobs tests started intermittently failing once a
//! third test claiming from the same table was added elsewhere.

use serde_json::Value;
use sqlx::PgPool;
use sqlx::Row;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum JobsError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

pub type Result<T> = std::result::Result<T, JobsError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    fn from_str(s: &str) -> Self {
        match s {
            "running" => JobStatus::Running,
            "succeeded" => JobStatus::Succeeded,
            "failed" => JobStatus::Failed,
            "cancelled" => JobStatus::Cancelled,
            _ => JobStatus::Pending,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: Uuid,
    pub job_type: String,
    pub idempotency_key: String,
    pub payload: Value,
    pub status: JobStatus,
    pub attempts: i32,
    pub max_attempts: i32,
}

/// Enqueue a job. If a job with the same `(job_type, idempotency_key)`
/// already exists, returns the existing job's id instead of creating a
/// duplicate (spec §14.1: idempotency keys for mutating background actions).
pub async fn enqueue(
    pool: &PgPool,
    job_type: &str,
    idempotency_key: &str,
    payload: Value,
) -> Result<Uuid> {
    let row = sqlx::query(
        r#"
        insert into jobs (job_type, idempotency_key, payload)
        values ($1, $2, $3)
        on conflict (job_type, idempotency_key) do update
            set updated_at = now()
        returning id
        "#,
    )
    .bind(job_type)
    .bind(idempotency_key)
    .bind(payload)
    .fetch_one(pool)
    .await?;

    Ok(row.get::<Uuid, _>("id"))
}

/// Claim up to one pending, due job for the given worker using
/// `FOR UPDATE SKIP LOCKED` so concurrent workers never contend on the same
/// row, and grant it a lease of `lease_secs`.
pub async fn claim(pool: &PgPool, worker_id: &str, lease_secs: i64) -> Result<Option<Job>> {
    let mut tx = pool.begin().await?;

    let row = sqlx::query(
        r#"
        select id, job_type, idempotency_key, payload, status, attempts, max_attempts
        from jobs
        where status = 'pending' and run_at <= now()
        order by run_at
        for update skip locked
        limit 1
        "#,
    )
    .fetch_optional(&mut *tx)
    .await?;

    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };

    let id: Uuid = row.get("id");

    sqlx::query(
        r#"
        update jobs
        set status = 'running',
            locked_by = $2,
            locked_at = now(),
            lease_expires_at = now() + make_interval(secs => $3),
            attempts = attempts + 1,
            updated_at = now()
        where id = $1
        "#,
    )
    .bind(id)
    .bind(worker_id)
    .bind(lease_secs as f64)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Some(Job {
        id,
        job_type: row.get("job_type"),
        idempotency_key: row.get("idempotency_key"),
        payload: row.get("payload"),
        status: JobStatus::Running,
        attempts: row.get::<i32, _>("attempts") + 1,
        max_attempts: row.get("max_attempts"),
    }))
}

/// Renew a claimed job's lease and optionally report progress. Callers
/// should heartbeat well before `lease_secs` elapses so another worker
/// doesn't reclaim the job as abandoned.
pub async fn heartbeat(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    lease_secs: i64,
    progress: Option<Value>,
) -> Result<bool> {
    let result = sqlx::query(
        r#"
        update jobs
        set lease_expires_at = now() + make_interval(secs => $3),
            progress = coalesce($4, progress),
            updated_at = now()
        where id = $1 and locked_by = $2 and status = 'running'
        "#,
    )
    .bind(job_id)
    .bind(worker_id)
    .bind(lease_secs as f64)
    .bind(progress)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Mark a job succeeded.
pub async fn complete(pool: &PgPool, job_id: Uuid, worker_id: &str) -> Result<bool> {
    let result = sqlx::query(
        r#"
        update jobs
        set status = 'succeeded', updated_at = now()
        where id = $1 and locked_by = $2
        "#,
    )
    .bind(job_id)
    .bind(worker_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Fail a job. If attempts remain, re-queues with exponential backoff;
/// otherwise marks it permanently failed.
pub async fn fail(pool: &PgPool, job_id: Uuid, worker_id: &str, error: &str) -> Result<bool> {
    let row =
        sqlx::query(r#"select attempts, max_attempts from jobs where id = $1 and locked_by = $2"#)
            .bind(job_id)
            .bind(worker_id)
            .fetch_optional(pool)
            .await?;

    let Some(row) = row else {
        return Ok(false);
    };

    let attempts: i32 = row.get("attempts");
    let max_attempts: i32 = row.get("max_attempts");

    if attempts >= max_attempts {
        sqlx::query(
            r#"
            update jobs
            set status = 'failed', last_error = $3, updated_at = now()
            where id = $1 and locked_by = $2
            "#,
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(error)
        .execute(pool)
        .await?;
    } else {
        // Exponential backoff: 2^attempts seconds, capped at 1 hour.
        let backoff_secs = (2i64.saturating_pow(attempts.max(0) as u32)).min(3600);

        sqlx::query(
            r#"
            update jobs
            set status = 'pending',
                run_at = now() + make_interval(secs => $3),
                locked_by = null,
                locked_at = null,
                lease_expires_at = null,
                last_error = $4,
                updated_at = now()
            where id = $1 and locked_by = $2
            "#,
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(backoff_secs as f64)
        .bind(error)
        .execute(pool)
        .await?;
    }

    Ok(true)
}

/// Mark a job permanently failed without going through the retry/backoff
/// logic in [`fail`] — for cases where retrying can never succeed, such
/// as an unrecognized job type. Regardless of attempts/max_attempts.
pub async fn fail_permanently(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    error: &str,
) -> Result<bool> {
    let result = sqlx::query(
        r#"
        update jobs
        set status = 'failed', last_error = $3, updated_at = now()
        where id = $1 and locked_by = $2
        "#,
    )
    .bind(job_id)
    .bind(worker_id)
    .bind(error)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Request cancellation of a job; the worker executing it is expected to
/// poll `cancel_requested` and stop cooperatively.
pub async fn request_cancel(pool: &PgPool, job_id: Uuid) -> Result<bool> {
    let result =
        sqlx::query(r#"update jobs set cancel_requested = true, updated_at = now() where id = $1"#)
            .bind(job_id)
            .execute(pool)
            .await?;

    Ok(result.rows_affected() > 0)
}

/// Reclaim jobs whose lease has expired without a heartbeat, returning them
/// to `pending` so another worker can pick them up.
pub async fn reap_expired_leases(pool: &PgPool) -> Result<u64> {
    let result = sqlx::query(
        r#"
        update jobs
        set status = 'pending',
            locked_by = null,
            locked_at = null,
            lease_expires_at = null,
            updated_at = now()
        where status = 'running' and lease_expires_at < now()
        "#,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

pub async fn get(pool: &PgPool, job_id: Uuid) -> Result<Option<Job>> {
    let row = sqlx::query(
        r#"
        select id, job_type, idempotency_key, payload, status, attempts, max_attempts
        from jobs where id = $1
        "#,
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| Job {
        id: row.get("id"),
        job_type: row.get("job_type"),
        idempotency_key: row.get("idempotency_key"),
        payload: row.get("payload"),
        status: JobStatus::from_str(row.get::<String, _>("status").as_str()),
        attempts: row.get("attempts"),
        max_attempts: row.get("max_attempts"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::env;

    /// Integration test gated on `DATABASE_URL`; skipped otherwise so
    /// `cargo test --workspace` never requires a live database.
    async fn pool_or_skip() -> Option<PgPool> {
        let url = env::var("DATABASE_URL").ok()?;
        Some(
            PgPool::connect(&url)
                .await
                .expect("connect to DATABASE_URL"),
        )
    }

    #[tokio::test]
    async fn enqueue_claim_complete_roundtrip() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();

        let key = Uuid::new_v4().to_string();
        let id = enqueue(&pool, "test_job", &key, json!({"n": 1}))
            .await
            .unwrap();

        // Enqueue again with same idempotency key: must not duplicate.
        let id2 = enqueue(&pool, "test_job", &key, json!({"n": 1}))
            .await
            .unwrap();
        assert_eq!(id, id2);

        let claimed = claim(&pool, "worker-1", 30).await.unwrap().unwrap();
        assert_eq!(claimed.id, id);
        assert_eq!(claimed.status, JobStatus::Running);

        // A second worker must not be able to claim the same job.
        // (No other pending jobs exist, so claim() returns None.)
        assert!(claim(&pool, "worker-2", 30).await.unwrap().is_none());

        let hb_ok = heartbeat(&pool, id, "worker-1", 30, Some(json!({"pct": 50})))
            .await
            .unwrap();
        assert!(hb_ok);

        let completed = complete(&pool, id, "worker-1").await.unwrap();
        assert!(completed);

        let job = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Succeeded);
    }

    #[tokio::test]
    async fn fail_retries_then_permanently_fails() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();

        let key = Uuid::new_v4().to_string();
        let id = enqueue(&pool, "flaky_job", &key, json!({})).await.unwrap();

        // Force max_attempts = 1 so the first failure is terminal.
        sqlx::query("update jobs set max_attempts = 1 where id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();

        let claimed = claim(&pool, "worker-1", 30).await.unwrap().unwrap();
        assert_eq!(claimed.id, id);

        fail(&pool, id, "worker-1", "boom").await.unwrap();

        let job = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Failed);
    }

    #[tokio::test]
    async fn fail_permanently_ignores_remaining_attempts() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();

        let key = Uuid::new_v4().to_string();
        let id = enqueue(&pool, "unknown_kind", &key, json!({}))
            .await
            .unwrap();

        let claimed = claim(&pool, "worker-1", 30).await.unwrap().unwrap();
        assert_eq!(claimed.attempts, 1);
        assert!(
            claimed.max_attempts > 1,
            "sanity: retries would normally remain"
        );

        fail_permanently(&pool, id, "worker-1", "unknown job kind")
            .await
            .unwrap();

        let job = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Failed);
    }
}
