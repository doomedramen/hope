//! Enrollment tokens and agent identity records (ADR-0007). Tokens are
//! stored only as a SHA-256 hash; the plaintext is shown to the operator
//! exactly once (CLI stdout) and never logged.

use sha2::{Digest, Sha256};
use sqlx::PgPool;
use sqlx::Row;
use uuid::Uuid;

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

pub struct AgentRecord {
    pub id: Uuid,
    pub revoked_at: Option<time::OffsetDateTime>,
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
    sqlx::query(r#"update agents set last_seen = now() where id = $1"#)
        .bind(agent_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn revoke(pool: &PgPool, agent_id: Uuid) -> sqlx::Result<()> {
    sqlx::query(r#"update agents set revoked_at = now() where id = $1"#)
        .bind(agent_id)
        .execute(pool)
        .await?;
    Ok(())
}
