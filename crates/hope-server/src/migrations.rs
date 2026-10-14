//! Ordered, transactional migrations, including adoption of pre-migration databases.
use anyhow::{Result, ensure};
use sqlx::SqlitePool;

pub async fn run(db: &SqlitePool) -> Result<()> {
    let mut tx = db.begin().await?;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *tx)
        .await?;
    let migrations = [
        (1, include_str!("schema.sql")),
        (2, include_str!("migrations/0002_statistics_and_reset.sql")),
        (3, include_str!("migrations/0003_install_jobs.sql")),
    ];
    ensure!(version <= 3, "Database version is newer than this server");
    for (target, source) in migrations {
        if version < target {
            sqlx::raw_sql(source).execute(&mut *tx).await?;
            sqlx::query(&format!("PRAGMA user_version = {target}"))
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn adopts_legacy_database_without_losing_data_and_rejects_future_versions() {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("database");
        sqlx::raw_sql(include_str!("schema.sql"))
            .execute(&db)
            .await
            .expect("legacy schema");
        sqlx::query("INSERT INTO agents(id,credential_hash) VALUES('identity','hashed-secret')")
            .execute(&db)
            .await
            .expect("agent");
        sqlx::query("INSERT INTO metrics(agent_id,report_id,collected_at,body) VALUES('identity','sample',1,'{}')").execute(&db).await.expect("history");
        run(&db).await.expect("upgrade");
        run(&db).await.expect("idempotent upgrade");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metrics")
            .fetch_one(&db)
            .await
            .expect("count");
        assert_eq!(count, 1);
        let credential: String = sqlx::query_scalar("SELECT credential_hash FROM agents")
            .fetch_one(&db)
            .await
            .expect("identity");
        assert_eq!(credential, "hashed-secret");
        sqlx::query("PRAGMA user_version=999")
            .execute(&db)
            .await
            .expect("future version");
        assert!(run(&db).await.is_err());
    }
}
