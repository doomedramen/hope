//! `change_events` retention (design §5, Decision 5): bounded-batch
//! delete of rows older than a configurable window, run as a worker job
//! so it goes through the same registry/retry machinery as every other
//! job (ADR-0006) instead of a bespoke timer.

use sqlx::PgPool;

const BATCH_SIZE: i64 = 1000;

/// Delete `change_events` older than `retention_days`, in bounded
/// `LIMIT`-ed batches (via a `ctid` subquery) so this never holds one
/// long-running transaction or table lock regardless of table size.
/// Returns the total number of rows deleted.
pub async fn purge_old_change_events(pool: &PgPool, retention_days: i64) -> sqlx::Result<u64> {
    let mut total = 0u64;
    loop {
        let result = sqlx::query(
            "delete from change_events where ctid in ( \
                select ctid from change_events \
                where occurred_at < now() - ($1 || ' days')::interval \
                limit $2 \
             )",
        )
        .bind(retention_days)
        .bind(BATCH_SIZE)
        .execute(pool)
        .await?;

        let deleted = result.rows_affected();
        total += deleted;
        if deleted < BATCH_SIZE as u64 {
            break;
        }
    }
    Ok(total)
}

/// Aggregate unrolled monitor observations into hourly buckets. The worker
/// claims raw rows in bounded batches, but each upsert replaces the complete
/// bucket aggregate so a bucket split across batches remains correct.
pub async fn rollup_monitor_results(pool: &PgPool, rollup_after_days: i64) -> sqlx::Result<u64> {
    let rollup_after_days = rollup_after_days.max(1);
    let mut total = 0u64;
    loop {
        let (rolled_up,): (i64,) = sqlx::query_as(
            r#"
                with candidates as (
                    select id, monitor_id, date_trunc('hour', observed_at) as bucket_start
                    from monitor_results
                    where rolled_up_at is null
                      and observed_at < now() - ($1 || ' days')::interval
                    order by observed_at, id
                    for update skip locked
                    limit $2
                ), buckets as (
                    select distinct monitor_id, bucket_start from candidates
                ), upserted as (
                    insert into monitor_result_rollups
                        (monitor_id, bucket_start, sample_count, success_count, failure_count,
                         timeout_count, error_count, min_latency_ms, avg_latency_ms,
                         max_latency_ms, last_status, updated_at)
                    select r.monitor_id, b.bucket_start, count(*),
                           count(*) filter (where r.status = 'success'),
                           count(*) filter (where r.status = 'failure'),
                           count(*) filter (where r.status = 'timeout'),
                           count(*) filter (where r.status = 'error'),
                           min(r.latency_ms), avg(r.latency_ms::double precision),
                           max(r.latency_ms),
                           (array_agg(r.status order by r.observed_at desc, r.id desc))[1], now()
                    from monitor_results r
                    join buckets b on b.monitor_id = r.monitor_id
                                  and b.bucket_start = date_trunc('hour', r.observed_at)
                    group by r.monitor_id, b.bucket_start
                    on conflict (monitor_id, bucket_start) do update set
                        sample_count = excluded.sample_count,
                        success_count = excluded.success_count,
                        failure_count = excluded.failure_count,
                        timeout_count = excluded.timeout_count,
                        error_count = excluded.error_count,
                        min_latency_ms = excluded.min_latency_ms,
                        avg_latency_ms = excluded.avg_latency_ms,
                        max_latency_ms = excluded.max_latency_ms,
                        last_status = excluded.last_status,
                        updated_at = excluded.updated_at
                    returning monitor_id
                ), marked as (
                    update monitor_results r
                    set rolled_up_at = now()
                    from candidates c
                    where r.id = c.id and exists (select 1 from upserted)
                    returning r.id
                )
                select count(*) from marked
            "#,
        )
        .bind(rollup_after_days)
        .bind(BATCH_SIZE)
        .fetch_one(pool)
        .await?;
        total += u64::try_from(rolled_up).unwrap_or_default();
        if rolled_up < BATCH_SIZE {
            break;
        }
    }
    Ok(total)
}

/// Delete raw monitor results only after they have been included in a rollup.
/// A failed or delayed rollup therefore retains the raw observation instead of
/// silently losing history.
pub async fn purge_old_monitor_results(pool: &PgPool, retention_days: i64) -> sqlx::Result<u64> {
    let retention_days = retention_days.max(1);
    let mut total = 0u64;
    loop {
        let result = sqlx::query(
            r#"
                delete from monitor_results
                where ctid in (
                    select ctid from monitor_results
                    where rolled_up_at is not null
                      and observed_at < now() - ($1 || ' days')::interval
                    order by observed_at, id
                    limit $2
                )
            "#,
        )
        .bind(retention_days)
        .bind(BATCH_SIZE)
        .execute(pool)
        .await?;
        let deleted = result.rows_affected();
        total += deleted;
        if deleted < BATCH_SIZE as u64 {
            break;
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;
    use uuid::Uuid;

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    #[tokio::test]
    async fn purges_only_rows_older_than_the_window() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        sqlx::query(
            "insert into change_events (entity_kind, entity_id, category, severity, occurred_at) \
             values ('devices', gen_random_uuid(), 'device', 'info', now() - interval '400 days')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into change_events (entity_kind, entity_id, category, severity, occurred_at) \
             values ('devices', gen_random_uuid(), 'device', 'info', now())",
        )
        .execute(&pool)
        .await
        .unwrap();

        let deleted = purge_old_change_events(&pool, 365).await.unwrap();
        assert!(deleted >= 1);

        let remaining_old: (i64,) = sqlx::query_as(
            "select count(*) from change_events where occurred_at < now() - interval '365 days'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining_old.0, 0);

        let remaining_recent: (i64,) = sqlx::query_as(
            "select count(*) from change_events where occurred_at > now() - interval '1 hour'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(remaining_recent.0 >= 1);
    }

    #[tokio::test]
    async fn rolls_up_old_monitor_results_before_purging_them() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        let device_id: (Uuid,) =
            sqlx::query_as("insert into devices (device_type) values ('unknown') returning id")
                .fetch_one(&pool)
                .await
                .unwrap();
        let service_id: (Uuid,) = sqlx::query_as(
            "insert into services (protocol, owner_kind, owner_id) \
             values ('tcp', 'device', $1) returning id",
        )
        .bind(device_id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        let endpoint_id: (Uuid,) = sqlx::query_as(
            "insert into endpoints (service_id, endpoint_type, address, port) \
             values ($1, 'socket', '127.0.0.1'::inet, 80) returning id",
        )
        .bind(service_id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        let monitor_id: (Uuid,) = sqlx::query_as(
            "insert into monitors (service_id, endpoint_id, monitor_type) \
             values ($1, $2, 'tcp') returning id",
        )
        .bind(service_id.0)
        .bind(endpoint_id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into monitor_results (monitor_id, status, observed_at, latency_ms) \
             values ($1, 'success', now() - interval '10 days', 10),\
                    ($1, 'failure', now() - interval '10 days' + interval '1 minute', 20)",
        )
        .bind(monitor_id.0)
        .execute(&pool)
        .await
        .unwrap();

        assert!(rollup_monitor_results(&pool, 1).await.unwrap() >= 2);
        let rollup: (i64, i64, i64) = sqlx::query_as(
            "select sample_count, success_count, failure_count \
             from monitor_result_rollups where monitor_id = $1",
        )
        .bind(monitor_id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rollup, (2, 1, 1));

        assert!(purge_old_monitor_results(&pool, 5).await.unwrap() >= 2);
        let remaining: (i64,) =
            sqlx::query_as("select count(*) from monitor_results where monitor_id = $1")
                .bind(monitor_id.0)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(remaining.0, 0);
    }
}
