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
