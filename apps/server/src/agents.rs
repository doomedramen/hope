//! Enrollment tokens and agent identity records (ADR-0007). Tokens are
//! stored only as a SHA-256 hash; the plaintext is shown to the operator
//! exactly once (CLI stdout) and never logged.

use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

pub const HEARTBEAT_TIMEOUT_SECONDS: i64 = 90;

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

fn generate_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Create a single-use enrollment token, valid for `ttl_minutes`. Returns
/// the plaintext token — the only time it is ever available in cleartext.
pub async fn create_enrollment_token(pool: &PgPool, ttl_minutes: i64) -> sqlx::Result<String> {
    let token = generate_token();
    let hash = hash_token(&token);

    sqlx::query(
        r#"
        insert into enrollment_tokens (token_hash, expires_at)
        values ($1, now() + make_interval(mins => $2))
        "#,
    )
    .bind(&hash)
    .bind(ttl_minutes as i32)
    .execute(pool)
    .await?;

    Ok(token)
}

/// Atomically consume a token: succeeds only if it exists, is unexpired,
/// and unused. Marks it used in the same statement so a replayed token can
/// never succeed twice, even under concurrent requests.
pub async fn consume_enrollment_token(pool: &PgPool, token: &str) -> sqlx::Result<bool> {
    let hash = hash_token(token);

    let result = sqlx::query(
        r#"
        update enrollment_tokens
        set used_at = now()
        where token_hash = $1
          and used_at is null
          and expires_at > now()
        "#,
    )
    .bind(&hash)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() == 1)
}

/// Delete enrollment tokens that are no longer useful to keep around:
/// already expired (whether or not they were used), or already used
/// (regardless of expiry — a consumed token has no further purpose).
/// Returns how many rows were removed. Intended to run as a periodic
/// scheduled job (see `scheduler.rs` / `jobs_handlers.rs`), not on a
/// bespoke timer.
pub async fn purge_expired_tokens(pool: &PgPool) -> sqlx::Result<u64> {
    let result = sqlx::query(
        r#"delete from enrollment_tokens where expires_at < now() or used_at is not null"#,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub struct AgentRecord {
    pub id: Uuid,
    pub revoked_at: Option<time::OffsetDateTime>,
}

/// Update metadata learned from an authenticated Hello. The certificate-bound
/// agent id is supplied by the gateway, never taken from an untrusted hello.
pub struct HelloMetadata<'a> {
    pub agent_version: &'a str,
    pub hostname: &'a str,
    pub os: &'a str,
    pub arch: &'a str,
    pub protocol_version: i32,
    pub capabilities: &'a Value,
}

pub async fn update_hello(
    pool: &PgPool,
    agent_id: Uuid,
    metadata: &HelloMetadata<'_>,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "update agents set hostname = $2, agent_version = $3, os = $4, arch = $5, \
         protocol_version = $6, capabilities = $7, last_seen = now(), \
         last_heartbeat_at = now() where id = $1 and revoked_at is null",
    )
    .bind(agent_id)
    .bind(metadata.hostname)
    .bind(metadata.agent_version)
    .bind(metadata.os)
    .bind(metadata.arch)
    .bind(metadata.protocol_version)
    .bind(metadata.capabilities)
    .execute(&mut *tx)
    .await?;
    recover_health_incident(&mut tx, agent_id).await?;
    tx.commit().await
}

pub async fn insert_agent(
    pool: &PgPool,
    cert_fingerprint: &str,
    cert_serial: &str,
    hostname: Option<&str>,
) -> sqlx::Result<Uuid> {
    let row = sqlx::query(
        r#"
        insert into agents (cert_fingerprint, cert_serial, hostname)
        values ($1, $2, $3)
        returning id
        "#,
    )
    .bind(cert_fingerprint)
    .bind(cert_serial)
    .bind(hostname)
    .fetch_one(pool)
    .await?;

    Ok(row.get("id"))
}

pub async fn find_by_fingerprint(
    pool: &PgPool,
    cert_fingerprint: &str,
) -> sqlx::Result<Option<AgentRecord>> {
    let row = sqlx::query(r#"select id, revoked_at from agents where cert_fingerprint = $1"#)
        .bind(cert_fingerprint)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|row| AgentRecord {
        id: row.get("id"),
        revoked_at: row.get("revoked_at"),
    }))
}

pub async fn touch_last_seen(pool: &PgPool, agent_id: Uuid) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "update agents set last_seen = now(), last_heartbeat_at = now() \
         where id = $1 and revoked_at is null",
    )
    .bind(agent_id)
    .execute(&mut *tx)
    .await?;
    recover_health_incident(&mut tx, agent_id).await?;
    tx.commit().await
}

async fn recover_health_incident(
    tx: &mut Transaction<'_, Postgres>,
    agent_id: Uuid,
) -> sqlx::Result<()> {
    let recovered: Option<(Uuid,)> = sqlx::query_as(
        "update agent_health_incidents set state = 'recovered', recovered_at = now(), \
         last_event_at = now(), updated_at = now() \
         where agent_id = $1 and state = 'open' returning id",
    )
    .bind(agent_id)
    .fetch_optional(&mut **tx)
    .await?;

    if let Some((incident_id,)) = recovered {
        sqlx::query(
            "insert into change_events \
                (entity_kind, entity_id, category, severity, after, evidence_source) \
             values ('agents', $1, 'agent.heartbeat.recovered', 'notice', $2, 'agent')",
        )
        .bind(agent_id)
        .bind(serde_json::json!({
            "incident_id": incident_id,
            "state": "recovered",
        }))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Open or update durable offline incidents for agents that missed their
/// heartbeat deadline. Inventory tables are never touched by this sweep.
pub async fn sweep_offline(pool: &PgPool, timeout_seconds: i64) -> sqlx::Result<u64> {
    let fallback_timeout_seconds = timeout_seconds.clamp(30, 86_400);
    let candidate_ids: Vec<(Uuid, i32)> = sqlx::query_as(
        "select id, coalesce(nullif(heartbeat_timeout_seconds, 0), $1::integer) \
         from agents \
           where revoked_at is null \
           and coalesce(last_heartbeat_at, created_at) < now() - make_interval(secs => \
               coalesce(nullif(heartbeat_timeout_seconds, 0), $1::integer)::double precision)",
    )
    .bind(fallback_timeout_seconds as i32)
    .fetch_all(pool)
    .await?;

    let mut opened = 0;
    for (agent_id, agent_timeout_seconds) in candidate_ids {
        let mut tx = pool.begin().await?;
        let still_offline: Option<(Uuid,)> = sqlx::query_as(
            "select id from agents where id = $1 and revoked_at is null \
             and coalesce(last_heartbeat_at, created_at) < \
                 now() - make_interval(secs => $2::double precision) for update",
        )
        .bind(agent_id)
        .bind(agent_timeout_seconds as f64)
        .fetch_optional(&mut *tx)
        .await?;

        if still_offline.is_none() {
            tx.commit().await?;
            continue;
        }

        let existing: Option<(Uuid,)> = sqlx::query_as(
            "select id from agent_health_incidents \
             where agent_id = $1 and state = 'open' for update",
        )
        .bind(agent_id)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some((incident_id,)) = existing {
            sqlx::query(
                "update agent_health_incidents set failure_count = failure_count + 1, \
                 last_event_at = now(), updated_at = now(), details = jsonb_set(details, \
                 '{timeout_seconds}', to_jsonb($2::integer), true) where id = $1",
            )
            .bind(incident_id)
            .bind(agent_timeout_seconds)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                "insert into agent_health_incidents \
                    (agent_id, severity, summary, details) \
                 values ($1, 'warning', 'agent heartbeat missed', \
                         jsonb_build_object('timeout_seconds', $2::integer))",
            )
            .bind(agent_id)
            .bind(agent_timeout_seconds)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "insert into change_events \
                    (entity_kind, entity_id, category, severity, after, evidence_source) \
                 values ('agents', $1, 'agent.heartbeat.offline', 'warning', $2, 'system')",
            )
            .bind(agent_id)
            .bind(serde_json::json!({
                "state": "open",
                "timeout_seconds": agent_timeout_seconds,
            }))
            .execute(&mut *tx)
            .await?;
            opened += 1;
        }
        tx.commit().await?;
    }

    Ok(opened)
}

pub async fn revoke(pool: &PgPool, agent_id: Uuid) -> sqlx::Result<()> {
    sqlx::query(r#"update agents set revoked_at = now() where id = $1"#)
        .bind(agent_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");
        Some(pool)
    }

    #[tokio::test]
    async fn sweep_offline_uses_each_agents_configured_timeout() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        let fast_id: Uuid = sqlx::query_scalar(
            "insert into agents \
                (cert_fingerprint, cert_serial, heartbeat_timeout_seconds, last_heartbeat_at) \
             values ($1, $2, 30, now() - interval '2 minutes') returning id",
        )
        .bind(format!("agent-health-fast-{}", Uuid::new_v4()))
        .bind(format!("serial-fast-{}", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .expect("insert fast-timeout agent");
        let slow_id: Uuid = sqlx::query_scalar(
            "insert into agents \
                (cert_fingerprint, cert_serial, heartbeat_timeout_seconds, last_heartbeat_at) \
             values ($1, $2, 300, now() - interval '2 minutes') returning id",
        )
        .bind(format!("agent-health-slow-{}", Uuid::new_v4()))
        .bind(format!("serial-slow-{}", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .expect("insert slow-timeout agent");

        sweep_offline(&pool, HEARTBEAT_TIMEOUT_SECONDS)
            .await
            .expect("sweep offline agents");

        let fast_state: Option<String> = sqlx::query_scalar(
            "select state from agent_health_incidents \
             where agent_id = $1 and state = 'open'",
        )
        .bind(fast_id)
        .fetch_optional(&pool)
        .await
        .expect("read fast-timeout incident");
        let slow_state: Option<String> = sqlx::query_scalar(
            "select state from agent_health_incidents \
             where agent_id = $1 and state = 'open'",
        )
        .bind(slow_id)
        .fetch_optional(&pool)
        .await
        .expect("read slow-timeout incident");

        assert_eq!(fast_state.as_deref(), Some("open"));
        assert_eq!(slow_state, None);

        sqlx::query("delete from change_events where entity_id in ($1, $2)")
            .bind(fast_id)
            .bind(slow_id)
            .execute(&pool)
            .await
            .expect("delete test change events");
        sqlx::query("delete from agents where id in ($1, $2)")
            .bind(fast_id)
            .bind(slow_id)
            .execute(&pool)
            .await
            .expect("delete test agents");
    }
}
