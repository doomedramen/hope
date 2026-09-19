//! Typed job-kind -> handler registry for the worker role. Unregistered
//! kinds are a configuration/deploy error (a job enqueued by a newer
//! server talking to an older worker, or a typo), not something retrying
//! could ever fix — the worker fails them permanently with a clear
//! message rather than retrying forever (see `jobs::fail_permanently`).

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;
use sqlx::PgPool;

use crate::agents;
use crate::session_store::PgSessionStore;

#[async_trait]
pub trait JobHandler: Send + Sync {
    async fn handle(&self, pool: &PgPool, payload: Value) -> anyhow::Result<()>;
}

/// Delete expired session rows (spec §12.3 secure sessions). Previously a
/// bespoke hourly `tokio::spawn` loop in `main.rs`; moved to a scheduled
/// job so it goes through the same retry/lease/observability machinery as
/// everything else the worker does.
struct SessionCleanup;

#[async_trait]
impl JobHandler for SessionCleanup {
    async fn handle(&self, pool: &PgPool, _payload: Value) -> anyhow::Result<()> {
        let store = PgSessionStore::new(pool.clone());
        let deleted = store.delete_expired().await?;
        if deleted > 0 {
            tracing::info!(count = deleted, "deleted expired sessions");
        }
        Ok(())
    }
}

/// Delete enrollment tokens that are expired or already used (ADR-0007).
struct EnrollmentTokenPurge;

#[async_trait]
impl JobHandler for EnrollmentTokenPurge {
    async fn handle(&self, pool: &PgPool, _payload: Value) -> anyhow::Result<()> {
        let deleted = agents::purge_expired_tokens(pool).await?;
        if deleted > 0 {
            tracing::info!(count = deleted, "purged expired/used enrollment tokens");
        }
        Ok(())
    }
}

/// No-op handler for exercising the queue/worker pipeline end to end
/// (enqueue -> claim -> dispatch -> complete) without any real side
/// effect. Logs its payload so a test/operator can confirm it actually
/// ran.
struct DiagnosticEcho;

#[async_trait]
impl JobHandler for DiagnosticEcho {
    async fn handle(&self, _pool: &PgPool, payload: Value) -> anyhow::Result<()> {
        tracing::info!(?payload, "diagnostic.echo");
        Ok(())
    }
}

pub struct Registry(HashMap<&'static str, Box<dyn JobHandler>>);

impl Registry {
    pub fn new() -> Self {
        let mut handlers: HashMap<&'static str, Box<dyn JobHandler>> = HashMap::new();
        handlers.insert("session.cleanup", Box::new(SessionCleanup));
        handlers.insert("enrollment_token.purge", Box::new(EnrollmentTokenPurge));
        handlers.insert("diagnostic.echo", Box::new(DiagnosticEcho));
        Self(handlers)
    }

    pub fn get(&self, job_type: &str) -> Option<&dyn JobHandler> {
        self.0.get(job_type).map(|handler| handler.as_ref())
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
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
    async fn registry_dispatches_known_kinds_and_rejects_unknown() {
        let registry = Registry::new();
        assert!(registry.get("session.cleanup").is_some());
        assert!(registry.get("enrollment_token.purge").is_some());
        assert!(registry.get("diagnostic.echo").is_some());
        assert!(registry.get("no.such.kind").is_none());
    }

    #[tokio::test]
    async fn diagnostic_echo_always_succeeds() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let registry = Registry::new();
        let handler = registry.get("diagnostic.echo").unwrap();
        handler
            .handle(&pool, serde_json::json!({"hello": "world"}))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn session_cleanup_handler_deletes_expired_sessions() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        sqlx::query(
            "insert into sessions (id, data, expiry_date) values ($1, '{}'::jsonb, now() - interval '1 hour')",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .execute(&pool)
        .await
        .unwrap();

        let registry = Registry::new();
        let handler = registry.get("session.cleanup").unwrap();
        handler.handle(&pool, serde_json::json!({})).await.unwrap();

        let remaining: (i64,) =
            sqlx::query_as("select count(*) from sessions where expiry_date < now()")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(remaining.0, 0);
    }

    #[tokio::test]
    async fn enrollment_token_purge_handler_deletes_expired_and_used_tokens() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        sqlx::query(
            "insert into enrollment_tokens (token_hash, expires_at) values ($1, now() - interval '1 hour')",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .execute(&pool)
        .await
        .unwrap();

        let registry = Registry::new();
        let handler = registry.get("enrollment_token.purge").unwrap();
        handler.handle(&pool, serde_json::json!({})).await.unwrap();

        let remaining: (i64,) =
            sqlx::query_as("select count(*) from enrollment_tokens where expires_at < now()")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(remaining.0, 0);
    }
}
