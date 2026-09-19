//! Postgres-backed `tower_sessions::SessionStore` (spec §12.3: secure
//! session cookies). `tower-sessions-sqlx-store` 0.15.0 depends on
//! `tower-sessions-core` 0.14, which is a different (incompatible) type
//! from the `tower-sessions-core` 0.15 our `tower-sessions` 0.15 pulls in
//! -- its `SessionStore` impl doesn't satisfy our `SessionManagerLayer`.
//! Hand-rolled instead: a thin wrapper around one `sessions` table.

use async_trait::async_trait;
use sqlx::PgPool;
use sqlx::Row;
use time::OffsetDateTime;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, SessionStore};

#[derive(Clone, Debug)]
pub struct PgSessionStore {
    pool: PgPool,
}

impl PgSessionStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Delete all expired sessions, returning how many were removed.
    /// Intended to be called on a periodic background loop (see
    /// `main.rs`); not part of the `SessionStore` trait itself.
    pub async fn delete_expired(&self) -> sqlx::Result<u64> {
        let result = sqlx::query("delete from sessions where expiry_date <= now()")
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

fn backend_err(err: sqlx::Error) -> session_store::Error {
    session_store::Error::Backend(err.to_string())
}

#[async_trait]
impl SessionStore for PgSessionStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        // Mirror the default_create collision-retry behaviour but detect
        // the collision via the primary key conflict itself rather than a
        // separate existence check (avoids a TOCTOU race).
        loop {
            let data = serde_json::to_value(&record.data)
                .map_err(|err| session_store::Error::Encode(err.to_string()))?;

            let result = sqlx::query(
                "insert into sessions (id, data, expiry_date) values ($1, $2, $3) \
                 on conflict (id) do nothing",
            )
            .bind(record.id.to_string())
            .bind(data)
            .bind(record.expiry_date)
            .execute(&self.pool)
            .await
            .map_err(backend_err)?;

            if result.rows_affected() == 1 {
                return Ok(());
            }
            record.id = Id::default();
        }
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let data = serde_json::to_value(&record.data)
            .map_err(|err| session_store::Error::Encode(err.to_string()))?;

        sqlx::query(
            "insert into sessions (id, data, expiry_date) values ($1, $2, $3) \
             on conflict (id) do update set data = excluded.data, expiry_date = excluded.expiry_date",
        )
        .bind(record.id.to_string())
        .bind(data)
        .bind(record.expiry_date)
        .execute(&self.pool)
        .await
        .map_err(backend_err)?;

        Ok(())
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        let row = sqlx::query(
            "select data, expiry_date from sessions where id = $1 and expiry_date > now()",
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(backend_err)?;

        let Some(row) = row else {
            return Ok(None);
        };

        let data_json: serde_json::Value = row.get("data");
        let expiry_date: OffsetDateTime = row.get("expiry_date");
        let data = serde_json::from_value(data_json)
            .map_err(|err| session_store::Error::Decode(err.to_string()))?;

        Ok(Some(Record {
            id: *session_id,
            data,
            expiry_date,
        }))
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        sqlx::query("delete from sessions where id = $1")
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(backend_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    /// Integration test gated on `DATABASE_URL`; skipped otherwise so
    /// `cargo test --workspace` never requires a live database.
    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    fn record_expiring_in(minutes: i64) -> Record {
        let mut record = Record {
            id: Id::default(),
            data: Default::default(),
            expiry_date: OffsetDateTime::now_utc() + Duration::minutes(minutes),
        };
        record.data.insert(
            "user_id".to_string(),
            serde_json::Value::String("test-user".to_string()),
        );
        record
    }

    #[tokio::test]
    async fn save_load_delete_roundtrip() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let store = PgSessionStore::new(pool);

        let mut record = record_expiring_in(10);
        store.create(&mut record).await.unwrap();

        let loaded = store.load(&record.id).await.unwrap().unwrap();
        assert_eq!(loaded.data, record.data);

        // save() updates in place (used when session data changes).
        record
            .data
            .insert("extra".to_string(), serde_json::Value::Bool(true));
        store.save(&record).await.unwrap();
        let reloaded = store.load(&record.id).await.unwrap().unwrap();
        assert_eq!(reloaded.data, record.data);

        store.delete(&record.id).await.unwrap();
        assert!(store.load(&record.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn create_avoids_id_collisions() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let store = PgSessionStore::new(pool);

        let mut first = record_expiring_in(10);
        store.create(&mut first).await.unwrap();

        // Force a collision: pre-seed a second record with the same id
        // that `create` would otherwise pick, then verify `create` gives
        // it a fresh id instead of clobbering the existing row.
        let mut second = record_expiring_in(10);
        second.id = first.id;
        store.create(&mut second).await.unwrap();

        assert_ne!(
            first.id, second.id,
            "collision must be retried with a new id"
        );
        let still_there = store.load(&first.id).await.unwrap().unwrap();
        assert_eq!(still_there.data, first.data);
    }

    #[tokio::test]
    async fn expired_session_is_not_loaded() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let store = PgSessionStore::new(pool);

        let mut record = record_expiring_in(-1); // already expired
        store.create(&mut record).await.unwrap();

        assert!(
            store.load(&record.id).await.unwrap().is_none(),
            "an expired session must not be returned by load()"
        );
    }

    #[tokio::test]
    async fn delete_expired_cleans_up_only_expired_rows() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let store = PgSessionStore::new(pool);

        let mut expired = record_expiring_in(-5);
        store.create(&mut expired).await.unwrap();
        let mut live = record_expiring_in(10);
        store.create(&mut live).await.unwrap();

        let deleted = store.delete_expired().await.unwrap();
        assert!(deleted >= 1, "should have deleted at least the expired row");

        // The row is gone even from a direct query (not just filtered by
        // load()'s expiry_date > now() clause) ...
        let row: Option<(String,)> = sqlx::query_as("select id from sessions where id = $1")
            .bind(expired.id.to_string())
            .fetch_optional(&store.pool)
            .await
            .unwrap();
        assert!(row.is_none());

        // ...while the still-live session survives.
        assert!(store.load(&live.id).await.unwrap().is_some());
    }
}
