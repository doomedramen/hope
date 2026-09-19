//! SSH host-key trust and change detection for M6.
//!
//! Host keys are public material, but accepting a changed key without an
//! explicit operator decision would make the installer vulnerable to a MITM
//! or host replacement. This module separates observation from trust.

use std::fmt;

use serde::Serialize;
use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::inventory::events::Recorder;

const MAX_HOST_LEN: usize = 253;
const MAX_KEY_TYPE_LEN: usize = 128;
const MAX_FINGERPRINT_LEN: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum SshTrustError {
    #[error("SSH host is invalid")]
    InvalidHost,
    #[error("SSH port is invalid")]
    InvalidPort,
    #[error("SSH key type is invalid")]
    InvalidKeyType,
    #[error("SSH host-key fingerprint is invalid")]
    InvalidFingerprint,
    #[error("SSH host-key state is invalid")]
    InvalidState,
    #[error("SSH host-key record was not found")]
    NotFound,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

pub type Result<T> = std::result::Result<T, SshTrustError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustState {
    Pending,
    Trusted,
    Changed,
    Revoked,
}

impl TrustState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Trusted => "trusted",
            Self::Changed => "changed",
            Self::Revoked => "revoked",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "trusted" => Ok(Self::Trusted),
            "changed" => Ok(Self::Changed),
            "revoked" => Ok(Self::Revoked),
            _ => Err(SshTrustError::InvalidState),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "decision")]
pub enum HostKeyDecision {
    FirstSeen {
        record: SshHostKeyMetadata,
    },
    Trusted {
        record: SshHostKeyMetadata,
    },
    Pending {
        record: SshHostKeyMetadata,
    },
    Changed {
        record: SshHostKeyMetadata,
        previous_fingerprint_sha256: String,
    },
    Revoked {
        record: SshHostKeyMetadata,
    },
}

impl HostKeyDecision {
    pub fn permits_connection(&self) -> bool {
        matches!(self, Self::Trusted { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SshHostKeyMetadata {
    pub id: Uuid,
    pub device_id: Option<Uuid>,
    pub host: String,
    pub port: i32,
    pub key_type: String,
    pub fingerprint_sha256: String,
    pub previous_fingerprint_sha256: Option<String>,
    pub state: TrustState,
    pub first_seen_at: time::OffsetDateTime,
    pub last_seen_at: time::OffsetDateTime,
    pub trusted_at: Option<time::OffsetDateTime>,
    pub changed_at: Option<time::OffsetDateTime>,
    pub revoked_at: Option<time::OffsetDateTime>,
    pub version: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PresentedHostKey {
    host: String,
    port: i32,
    key_type: String,
    fingerprint_sha256: String,
}

pub fn normalize_host(host: &str) -> Result<String> {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if host.is_empty()
        || host.len() > MAX_HOST_LEN
        || host
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || host.contains('/')
    {
        return Err(SshTrustError::InvalidHost);
    }
    Ok(host.to_ascii_lowercase())
}

pub fn validate_port(port: i32) -> Result<()> {
    if (1..=65_535).contains(&port) {
        Ok(())
    } else {
        Err(SshTrustError::InvalidPort)
    }
}

pub fn normalize_key_type(key_type: &str) -> Result<String> {
    let key_type = key_type.trim();
    if key_type.is_empty()
        || key_type.len() > MAX_KEY_TYPE_LEN
        || key_type
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(SshTrustError::InvalidKeyType);
    }
    Ok(key_type.to_string())
}

pub fn normalize_fingerprint(fingerprint: &str) -> Result<String> {
    let fingerprint = fingerprint.trim();
    let Some(encoded) = fingerprint.strip_prefix("SHA256:") else {
        return Err(SshTrustError::InvalidFingerprint);
    };
    if encoded.is_empty()
        || fingerprint.len() > MAX_FINGERPRINT_LEN
        || encoded.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '=' | '-' | '_'))
        })
    {
        return Err(SshTrustError::InvalidFingerprint);
    }
    Ok(format!("SHA256:{encoded}"))
}

/// Pure state decision used by the DB observation path and unit-tested
/// independently of PostgreSQL.
pub fn decide(existing: Option<(&str, TrustState)>, presented: &str) -> Result<TrustState> {
    let presented = normalize_fingerprint(presented)?;
    Ok(match existing {
        None => TrustState::Pending,
        Some((current, TrustState::Revoked)) if current == presented => TrustState::Revoked,
        Some((current, state)) if current == presented => state,
        Some(_) => TrustState::Changed,
    })
}

pub async fn observe(
    pool: &PgPool,
    device_id: Option<Uuid>,
    host: &str,
    port: i32,
    key_type: &str,
    fingerprint_sha256: &str,
    actor_user_id: Option<Uuid>,
) -> Result<HostKeyDecision> {
    let presented = PresentedHostKey {
        host: normalize_host(host)?,
        port: {
            validate_port(port)?;
            port
        },
        key_type: normalize_key_type(key_type)?,
        fingerprint_sha256: normalize_fingerprint(fingerprint_sha256)?,
    };
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "select id, device_id, host, port, key_type, fingerprint_sha256, \
                previous_fingerprint_sha256, state, first_seen_at, last_seen_at, \
                trusted_at, changed_at, revoked_at, version \
         from ssh_host_keys where host = $1 and port = $2 for update",
    )
    .bind(&presented.host)
    .bind(presented.port)
    .fetch_optional(&mut *tx)
    .await?;

    let decision = if let Some(row) = row {
        let current_fingerprint: String = row.try_get("fingerprint_sha256")?;
        let current_state = TrustState::parse(row.try_get("state")?)?;
        let state = decide(
            Some((&current_fingerprint, current_state)),
            &presented.fingerprint_sha256,
        )?;
        let id: Uuid = row.try_get("id")?;
        if state == TrustState::Changed {
            sqlx::query(
                "update ssh_host_keys set device_id = coalesce($2, device_id), key_type = $3, \
                 previous_fingerprint_sha256 = fingerprint_sha256, fingerprint_sha256 = $4, \
                 state = 'changed', changed_at = now(), last_seen_at = now(), updated_at = now(), version = version + 1 \
                 where id = $1",
            )
            .bind(id)
            .bind(device_id)
            .bind(&presented.key_type)
            .bind(&presented.fingerprint_sha256)
            .execute(&mut *tx)
            .await?;
            let metadata = get_in_transaction(&mut tx, id).await?;
            Recorder::record_audit(
                &mut tx,
                actor_user_id,
                "worker",
                "ssh_host_key.changed",
                Some("ssh_host_keys"),
                Some(id),
                "failure",
                Some(json!({
                    "host": metadata.host,
                    "port": metadata.port,
                    "previous_fingerprint_sha256": current_fingerprint,
                    "fingerprint_sha256": metadata.fingerprint_sha256,
                })),
            )
            .await?;
            HostKeyDecision::Changed {
                record: metadata,
                previous_fingerprint_sha256: current_fingerprint,
            }
        } else {
            sqlx::query(
                "update ssh_host_keys set device_id = coalesce($2, device_id), key_type = $3, \
                 last_seen_at = now(), updated_at = now() where id = $1",
            )
            .bind(id)
            .bind(device_id)
            .bind(&presented.key_type)
            .execute(&mut *tx)
            .await?;
            let metadata = get_in_transaction(&mut tx, id).await?;
            match state {
                TrustState::Trusted => HostKeyDecision::Trusted { record: metadata },
                TrustState::Revoked => HostKeyDecision::Revoked { record: metadata },
                _ => HostKeyDecision::Pending { record: metadata },
            }
        }
    } else {
        let row = sqlx::query(
            "insert into ssh_host_keys (device_id, host, port, key_type, fingerprint_sha256) \
             values ($1, $2, $3, $4, $5) returning id",
        )
        .bind(device_id)
        .bind(&presented.host)
        .bind(presented.port)
        .bind(&presented.key_type)
        .bind(&presented.fingerprint_sha256)
        .fetch_one(&mut *tx)
        .await?;
        let id: Uuid = row.try_get("id")?;
        let metadata = get_in_transaction(&mut tx, id).await?;
        Recorder::record_audit(
            &mut tx,
            actor_user_id,
            "worker",
            "ssh_host_key.first_seen",
            Some("ssh_host_keys"),
            Some(id),
            "success",
            Some(json!({ "host": metadata.host, "port": metadata.port })),
        )
        .await?;
        HostKeyDecision::FirstSeen { record: metadata }
    };
    tx.commit().await?;
    Ok(decision)
}

pub async fn trust(
    pool: &PgPool,
    id: Uuid,
    actor_user_id: Option<Uuid>,
) -> Result<SshHostKeyMetadata> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "update ssh_host_keys set state = 'trusted', trusted_at = now(), revoked_at = null, \
         changed_at = null, updated_at = now(), version = version + 1 where id = $1 \
         returning id, device_id, host, port, key_type, fingerprint_sha256, previous_fingerprint_sha256, \
                   state, first_seen_at, last_seen_at, trusted_at, changed_at, revoked_at, version",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(SshTrustError::NotFound)?;
    let metadata = metadata_from_row(&row)?;
    Recorder::record_audit(
        &mut tx,
        actor_user_id,
        "operator",
        "ssh_host_key.trust",
        Some("ssh_host_keys"),
        Some(id),
        "success",
        Some(json!({ "fingerprint_sha256": metadata.fingerprint_sha256 })),
    )
    .await?;
    tx.commit().await?;
    Ok(metadata)
}

pub async fn revoke(
    pool: &PgPool,
    id: Uuid,
    actor_user_id: Option<Uuid>,
) -> Result<SshHostKeyMetadata> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "update ssh_host_keys set state = 'revoked', revoked_at = now(), updated_at = now(), version = version + 1 \
         where id = $1 and revoked_at is null \
         returning id, device_id, host, port, key_type, fingerprint_sha256, previous_fingerprint_sha256, \
                   state, first_seen_at, last_seen_at, trusted_at, changed_at, revoked_at, version",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(SshTrustError::NotFound)?;
    let metadata = metadata_from_row(&row)?;
    Recorder::record_audit(
        &mut tx,
        actor_user_id,
        "operator",
        "ssh_host_key.revoke",
        Some("ssh_host_keys"),
        Some(id),
        "success",
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(metadata)
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<SshHostKeyMetadata>> {
    let row = sqlx::query(
        "select id, device_id, host, port, key_type, fingerprint_sha256, previous_fingerprint_sha256, \
                state, first_seen_at, last_seen_at, trusted_at, changed_at, revoked_at, version \
         from ssh_host_keys where id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| metadata_from_row(&row)).transpose()
}

async fn get_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<SshHostKeyMetadata> {
    let row = sqlx::query(
        "select id, device_id, host, port, key_type, fingerprint_sha256, previous_fingerprint_sha256, \
                state, first_seen_at, last_seen_at, trusted_at, changed_at, revoked_at, version \
         from ssh_host_keys where id = $1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    metadata_from_row(&row)
}

fn metadata_from_row(row: &sqlx::postgres::PgRow) -> Result<SshHostKeyMetadata> {
    Ok(SshHostKeyMetadata {
        id: row.try_get("id")?,
        device_id: row.try_get("device_id")?,
        host: row.try_get("host")?,
        port: row.try_get("port")?,
        key_type: row.try_get("key_type")?,
        fingerprint_sha256: row.try_get("fingerprint_sha256")?,
        previous_fingerprint_sha256: row.try_get("previous_fingerprint_sha256")?,
        state: TrustState::parse(row.try_get("state")?)?,
        first_seen_at: row.try_get("first_seen_at")?,
        last_seen_at: row.try_get("last_seen_at")?,
        trusted_at: row.try_get("trusted_at")?,
        changed_at: row.try_get("changed_at")?,
        revoked_at: row.try_get("revoked_at")?,
        version: row.try_get("version")?,
    })
}

impl fmt::Display for TrustState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP_A: &str = "SHA256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=";
    const FP_B: &str = "SHA256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb=";

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    #[test]
    fn normalizes_host_and_rejects_unsafe_values() {
        assert_eq!(normalize_host(" [HOST.Example] ").unwrap(), "host.example");
        assert!(normalize_host("host/path").is_err());
        assert!(normalize_host("host name").is_err());
        assert!(validate_port(22).is_ok());
        assert!(validate_port(0).is_err());
    }

    #[test]
    fn fingerprint_requires_sha256_prefix_and_bounded_alphabet() {
        assert_eq!(normalize_fingerprint(FP_A).unwrap(), FP_A);
        assert!(normalize_fingerprint("ssh-rsa AAAA").is_err());
        assert!(normalize_fingerprint("SHA256:bad fingerprint").is_err());
    }

    #[test]
    fn first_use_is_pending_and_trusted_match_remains_trusted() {
        assert_eq!(decide(None, FP_A).unwrap(), TrustState::Pending);
        assert_eq!(
            decide(Some((FP_A, TrustState::Trusted)), FP_A).unwrap(),
            TrustState::Trusted
        );
    }

    #[test]
    fn changed_and_revoked_keys_never_permit_connection() {
        assert_eq!(
            decide(Some((FP_A, TrustState::Trusted)), FP_B).unwrap(),
            TrustState::Changed
        );
        assert_eq!(
            decide(Some((FP_A, TrustState::Revoked)), FP_A).unwrap(),
            TrustState::Revoked
        );
        assert!(
            !HostKeyDecision::Changed {
                record: fake_metadata(TrustState::Changed),
                previous_fingerprint_sha256: FP_A.to_string(),
            }
            .permits_connection()
        );
    }

    #[tokio::test]
    async fn database_trust_flow_blocks_first_use_and_changes() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let host = format!("ssh-trust-test-{}.example", Uuid::new_v4());
        let first = observe(&pool, None, &host, 22, "ssh-ed25519", FP_A, None)
            .await
            .unwrap();
        let id = match first {
            HostKeyDecision::FirstSeen { record } => record.id,
            other => panic!("expected first-seen decision, got {other:?}"),
        };
        assert!(
            !observe(&pool, None, &host, 22, "ssh-ed25519", FP_A, None)
                .await
                .unwrap()
                .permits_connection()
        );
        assert_eq!(
            trust(&pool, id, None).await.unwrap().state,
            TrustState::Trusted
        );
        assert!(
            observe(&pool, None, &host, 22, "ssh-ed25519", FP_A, None)
                .await
                .unwrap()
                .permits_connection()
        );
        let changed = observe(&pool, None, &host, 22, "ssh-ed25519", FP_B, None)
            .await
            .unwrap();
        assert!(matches!(changed, HostKeyDecision::Changed { .. }));
        assert!(!changed.permits_connection());
        let trusted_replacement = trust(&pool, id, None).await.unwrap();
        assert_eq!(trusted_replacement.fingerprint_sha256, FP_B);
        assert!(revoke(&pool, id, None).await.is_ok());
        assert!(matches!(
            observe(&pool, None, &host, 22, "ssh-ed25519", FP_B, None)
                .await
                .unwrap(),
            HostKeyDecision::Revoked { .. }
        ));

        sqlx::query("delete from ssh_host_keys where id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }

    fn fake_metadata(state: TrustState) -> SshHostKeyMetadata {
        let now = time::OffsetDateTime::UNIX_EPOCH;
        SshHostKeyMetadata {
            id: Uuid::nil(),
            device_id: None,
            host: "host.example".to_string(),
            port: 22,
            key_type: "ssh-ed25519".to_string(),
            fingerprint_sha256: FP_B.to_string(),
            previous_fingerprint_sha256: Some(FP_A.to_string()),
            state,
            first_seen_at: now,
            last_seen_at: now,
            trusted_at: None,
            changed_at: None,
            revoked_at: None,
            version: 1,
        }
    }
}
