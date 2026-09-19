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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

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
}
