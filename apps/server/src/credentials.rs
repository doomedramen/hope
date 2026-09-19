//! M6 credential vault primitives.
//!
//! The database stores only authenticated ciphertext. The master key is
//! loaded from an operator-controlled environment variable or mounted file
//! and is never represented in a serializable application type. Callers use
//! [`CredentialStore::decrypt_for_agent_use`] only inside the short-lived worker
//! operation that needs the secret.

use std::fmt;
use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::inventory::events::Recorder;

const CIPHERTEXT_VERSION: u8 = 1;
const NONCE_LEN: usize = 12;
const MAX_NAME_BYTES: usize = 200;
const MAX_SCOPE_BYTES: usize = 8 * 1024;
const MAX_SECRET_BYTES: usize = 1024 * 1024;
const AAD_PREFIX: &[u8] = b"hope-credential-v1:";

/// Errors deliberately avoid interpolating secret or key material.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("credential master key is not configured")]
    MasterKeyMissing,
    #[error("credential master key must decode to exactly 32 bytes")]
    InvalidMasterKeyLength,
    #[error("credential master key has an invalid encoding")]
    InvalidMasterKeyEncoding,
    #[error("credential master key file could not be read")]
    MasterKeyFile(#[source] std::io::Error),
    #[error("credential input is invalid: {0}")]
    InvalidInput(&'static str),
    #[error("credential payload is too large")]
    PayloadTooLarge,
    #[error("credential ciphertext is invalid or has been tampered with")]
    InvalidCiphertext,
    #[error("credential secret cannot be decoded")]
    InvalidSecret,
    #[error("credential is unavailable")]
    Unavailable,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

pub type Result<T> = std::result::Result<T, CredentialError>;

/// A 256-bit key held in zeroizing memory. It has no `Serialize` or `Debug`
/// implementation that could accidentally put the key in logs.
#[derive(Clone)]
pub struct MasterKey(Zeroizing<[u8; 32]>);

impl MasterKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// Parse either a 64-character hex value or an RFC 4648 base64 value.
    /// Whitespace around a mounted-secret value is ignored, but the decoded
    /// value must still be exactly 32 bytes.
    pub fn from_encoded(encoded: &str) -> Result<Self> {
        let encoded = encoded.trim();
        let bytes = if encoded.len() == 64 && encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            hex::decode(encoded).map_err(|_| CredentialError::InvalidMasterKeyEncoding)?
        } else {
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
                .or_else(|_| {
                    base64::Engine::decode(
                        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                        encoded,
                    )
                })
                .map_err(|_| CredentialError::InvalidMasterKeyEncoding)?
        };

        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| CredentialError::InvalidMasterKeyLength)?;
        Ok(Self::from_bytes(bytes))
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = Zeroizing::new(std::fs::read(path).map_err(CredentialError::MasterKeyFile)?);
        let encoded =
            std::str::from_utf8(&bytes).map_err(|_| CredentialError::InvalidMasterKeyEncoding)?;
        Self::from_encoded(encoded)
    }

    /// Load the external key. An inline environment value takes precedence
    /// only when the file variable is absent; configuring both is rejected so
    /// deployments cannot silently use the wrong secret source.
    pub fn from_environment() -> Result<Self> {
        let inline = std::env::var("HOPE_CREDENTIAL_MASTER_KEY").ok();
        let file = std::env::var("HOPE_CREDENTIAL_MASTER_KEY_FILE").ok();
        match (inline, file) {
            (Some(_), Some(_)) => Err(CredentialError::InvalidInput(
                "set only one credential master-key source",
            )),
            (Some(value), None) => Self::from_encoded(&value),
            (None, Some(path)) => Self::from_file(path),
            (None, None) => Err(CredentialError::MasterKeyMissing),
        }
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MasterKey([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    SshPrivateKey,
    SshPassword,
}

impl CredentialKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::SshPrivateKey => "ssh_private_key",
            Self::SshPassword => "ssh_password",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "ssh_private_key" => Ok(Self::SshPrivateKey),
            "ssh_password" => Ok(Self::SshPassword),
            _ => Err(CredentialError::InvalidInput("unknown credential kind")),
        }
    }
}

/// The scope is explicit so callers cannot accidentally turn a selected
/// credential into a discovery-wide credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialScope {
    Device { device_id: Uuid },
    DeviceGroup { group_id: Uuid },
    Integration { integration_id: Uuid },
    Network { network_id: Uuid },
}

impl CredentialScope {
    fn validate(&self) -> Result<()> {
        let encoded =
            serde_json::to_vec(self).map_err(|_| CredentialError::InvalidInput("invalid scope"))?;
        if encoded.len() > MAX_SCOPE_BYTES {
            return Err(CredentialError::InvalidInput("scope is too large"));
        }
        Ok(())
    }
}

/// Secret input is accepted only by create/update operations. It deliberately
/// has a redacted `Debug` implementation and no `Serialize` implementation.
pub enum CredentialSecret {
    SshPrivateKey {
        username: Zeroizing<String>,
        private_key_pem: Zeroizing<String>,
        passphrase: Option<Zeroizing<String>>,
    },
    SshPassword {
        username: Zeroizing<String>,
        password: Zeroizing<String>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSshPrivateKey {
    username: Zeroizing<String>,
    private_key_pem: Zeroizing<String>,
    passphrase: Option<Zeroizing<String>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSshPassword {
    username: Zeroizing<String>,
    password: Zeroizing<String>,
}

impl fmt::Debug for CredentialSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialSecret([REDACTED])")
    }
}

impl CredentialSecret {
    fn kind(&self) -> CredentialKind {
        match self {
            Self::SshPrivateKey { .. } => CredentialKind::SshPrivateKey,
            Self::SshPassword { .. } => CredentialKind::SshPassword,
        }
    }

    fn validate(&self) -> Result<()> {
        let (username, size) = match self {
            Self::SshPrivateKey {
                username,
                private_key_pem,
                passphrase,
            } => (
                username,
                private_key_pem.len() + passphrase.as_deref().map_or(0, |value| value.len()),
            ),
            Self::SshPassword { username, password } => (username, password.len()),
        };
        if username.is_empty() || username.len() > 256 {
            return Err(CredentialError::InvalidInput(
                "SSH username length is invalid",
            ));
        }
        if size == 0 {
            return Err(CredentialError::InvalidInput("SSH secret is empty"));
        }
        if size > MAX_SECRET_BYTES {
            return Err(CredentialError::PayloadTooLarge);
        }
        Ok(())
    }

    fn encode(&self) -> Result<Zeroizing<Vec<u8>>> {
        self.validate()?;
        let value = match self {
            Self::SshPrivateKey {
                username,
                private_key_pem,
                passphrase,
            } => serde_json::to_vec(&StoredSshPrivateKey {
                username: username.clone(),
                private_key_pem: private_key_pem.clone(),
                passphrase: passphrase.clone(),
            })
            .map_err(|_| CredentialError::InvalidSecret)?,
            Self::SshPassword { username, password } => serde_json::to_vec(&StoredSshPassword {
                username: username.clone(),
                password: password.clone(),
            })
            .map_err(|_| CredentialError::InvalidSecret)?,
        };
        if value.len() > MAX_SECRET_BYTES {
            return Err(CredentialError::PayloadTooLarge);
        }
        Ok(Zeroizing::new(value))
    }

    fn decode(kind: CredentialKind, bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_SECRET_BYTES {
            return Err(CredentialError::PayloadTooLarge);
        }
        let secret = match kind {
            CredentialKind::SshPrivateKey => {
                let stored: StoredSshPrivateKey =
                    serde_json::from_slice(bytes).map_err(|_| CredentialError::InvalidSecret)?;
                Self::SshPrivateKey {
                    username: stored.username,
                    private_key_pem: stored.private_key_pem,
                    passphrase: stored.passphrase,
                }
            }
            CredentialKind::SshPassword => {
                let stored: StoredSshPassword =
                    serde_json::from_slice(bytes).map_err(|_| CredentialError::InvalidSecret)?;
                Self::SshPassword {
                    username: stored.username,
                    password: stored.password,
                }
            }
        };
        secret.validate()?;
        Ok(secret)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CredentialMetadata {
    pub id: Uuid,
    pub name: String,
    pub kind: CredentialKind,
    pub scope: CredentialScope,
    pub version: i32,
    pub created_by: Option<Uuid>,
    pub created_at: time::OffsetDateTime,
    pub updated_at: time::OffsetDateTime,
    pub last_used_at: Option<time::OffsetDateTime>,
    pub revoked_at: Option<time::OffsetDateTime>,
    pub deleted_at: Option<time::OffsetDateTime>,
}

#[derive(Debug)]
pub struct NewCredential {
    pub name: String,
    pub scope: CredentialScope,
    pub secret: CredentialSecret,
    pub created_by: Option<Uuid>,
}

#[derive(Debug)]
pub struct CredentialUpdate {
    pub name: Option<String>,
    pub scope: Option<CredentialScope>,
    pub secret: Option<CredentialSecret>,
    pub expected_version: i32,
}

#[derive(Clone)]
pub struct CredentialStore {
    pool: PgPool,
    key: MasterKey,
}

impl fmt::Debug for CredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialStore")
            .field("pool", &"[REDACTED]")
            .finish()
    }
}

impl CredentialStore {
    pub fn new(pool: PgPool, key: MasterKey) -> Self {
        Self { pool, key }
    }

    /// Construct a store from the external deployment secret. The key is
    /// parsed for each control-plane operation so a missing/invalid secret
    /// fails closed instead of silently falling back to an in-process default.
    pub fn from_environment(pool: PgPool) -> Result<Self> {
        Ok(Self::new(pool, MasterKey::from_environment()?))
    }

    pub async fn create(&self, input: NewCredential) -> Result<CredentialMetadata> {
        validate_name(&input.name)?;
        input.scope.validate()?;
        input.secret.validate()?;
        let id = Uuid::new_v4();
        let ciphertext = encrypt(&self.key, id, input.secret.kind(), &input.secret.encode()?)?;
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "insert into credentials (id, name, kind, scope, secret_ciphertext, created_by) \
             values ($1, $2, $3, $4, $5, $6) \
             returning id, name, kind, scope, version, created_by, created_at, updated_at, last_used_at, revoked_at, deleted_at",
        )
        .bind(id)
        .bind(&input.name)
        .bind(input.secret.kind().as_str())
        .bind(serde_json::to_value(&input.scope).map_err(|_| CredentialError::InvalidInput("invalid scope"))?)
        .bind(ciphertext)
        .bind(input.created_by)
        .fetch_one(&mut *tx)
        .await?;
        let metadata = metadata_from_row(&row)?;
        Recorder::record_audit(
            &mut tx,
            input.created_by,
            "operator",
            "credential.create",
            Some("credentials"),
            Some(id),
            "success",
            Some(json!({ "name": metadata.name, "kind": metadata.kind, "scope": metadata.scope })),
        )
        .await?;
        tx.commit().await?;
        Ok(metadata)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<CredentialMetadata>> {
        let row = sqlx::query(
            "select id, name, kind, scope, version, created_by, created_at, updated_at, last_used_at, revoked_at, deleted_at \
             from credentials where id = $1 and deleted_at is null",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| metadata_from_row(&row)).transpose()
    }

    pub async fn list(&self, limit: i64, cursor: Option<Uuid>) -> Result<Vec<CredentialMetadata>> {
        let limit = limit.clamp(1, 100);
        let rows = sqlx::query(
            "select id, name, kind, scope, version, created_by, created_at, updated_at, last_used_at, revoked_at, deleted_at \
             from credentials where deleted_at is null and ($1::uuid is null or id < $1) \
             order by id desc limit $2",
        )
        .bind(cursor)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(metadata_from_row).collect()
    }

    pub async fn update(
        &self,
        id: Uuid,
        actor_user_id: Option<Uuid>,
        input: CredentialUpdate,
    ) -> Result<CredentialMetadata> {
        if let Some(name) = &input.name {
            validate_name(name)?;
        }
        if let Some(scope) = &input.scope {
            scope.validate()?;
        }
        if let Some(secret) = &input.secret {
            secret.validate()?;
        }

        let mut tx = self.pool.begin().await?;
        let current = sqlx::query(
            "select kind from credentials where id = $1 and deleted_at is null and revoked_at is null and version = $2 for update",
        )
        .bind(id)
        .bind(input.expected_version)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(current) = current else {
            return Err(CredentialError::Unavailable);
        };
        let kind = CredentialKind::parse(current.get("kind"))?;
        let ciphertext = match input.secret {
            Some(secret) if secret.kind() != kind => {
                return Err(CredentialError::InvalidInput(
                    "credential kind cannot change",
                ));
            }
            Some(secret) => Some(encrypt(&self.key, id, kind, &secret.encode()?)?),
            None => None,
        };
        let row = sqlx::query(
            "update credentials set name = coalesce($2, name), scope = coalesce($3, scope), \
             secret_ciphertext = coalesce($4, secret_ciphertext), version = version + 1, updated_at = now() \
             where id = $1 and version = $5 \
             returning id, name, kind, scope, version, created_by, created_at, updated_at, last_used_at, revoked_at, deleted_at",
        )
        .bind(id)
        .bind(input.name)
        .bind(input.scope.map(serde_json::to_value).transpose().map_err(|_| CredentialError::InvalidInput("invalid scope"))?)
        .bind(ciphertext)
        .bind(input.expected_version)
        .fetch_one(&mut *tx)
        .await?;
        let metadata = metadata_from_row(&row)?;
        Recorder::record_audit(
            &mut tx,
            actor_user_id,
            "operator",
            "credential.update",
            Some("credentials"),
            Some(id),
            "success",
            Some(json!({ "version": metadata.version })),
        )
        .await?;
        tx.commit().await?;
        Ok(metadata)
    }

    pub async fn delete(&self, id: Uuid, actor_user_id: Option<Uuid>) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "update credentials set deleted_at = now(), revoked_at = coalesce(revoked_at, now()), updated_at = now() \
             where id = $1 and deleted_at is null",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 1 {
            Recorder::record_audit(
                &mut tx,
                actor_user_id,
                "operator",
                "credential.delete",
                Some("credentials"),
                Some(id),
                "success",
                None,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn disassociate_device(
        &self,
        credential_id: Uuid,
        device_id: Uuid,
        actor_user_id: Option<Uuid>,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "update credential_associations set disassociated_at = now() \
             where credential_id = $1 and device_id = $2 and purpose = 'agent_install' \
               and disassociated_at is null",
        )
        .bind(credential_id)
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 1 {
            Recorder::record_audit(
                &mut tx,
                actor_user_id,
                "operator",
                "credential.disassociate",
                Some("credentials"),
                Some(credential_id),
                "success",
                Some(json!({ "device_id": device_id, "purpose": "agent_install" })),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    /// Decrypt and mark a credential as used for one explicitly associated
    /// device. The returned secret is held in zeroizing strings only by the
    /// caller; this method never logs it.
    pub async fn decrypt_for_agent_use(
        &self,
        id: Uuid,
        device_id: Uuid,
        actor_user_id: Option<Uuid>,
    ) -> Result<CredentialSecret> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "select kind, scope, secret_ciphertext from credentials \
             where id = $1 and deleted_at is null and revoked_at is null for update",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(CredentialError::Unavailable)?;
        let kind = CredentialKind::parse(row.get("kind"))?;
        let scope: Value = row.try_get("scope")?;
        let scope: CredentialScope = serde_json::from_value(scope)
            .map_err(|_| CredentialError::InvalidInput("stored credential scope is invalid"))?;
        if !scope_allows_device(&scope, device_id) {
            return Err(CredentialError::Unavailable);
        }
        let associated: bool = sqlx::query_scalar(
            "select exists( \
                 select 1 from credential_associations \
                  where credential_id = $1 and device_id = $2 \
                    and purpose = 'agent_install' and disassociated_at is null \
             )",
        )
        .bind(id)
        .bind(device_id)
        .fetch_one(&mut *tx)
        .await?;
        if !associated {
            return Err(CredentialError::Unavailable);
        }
        let ciphertext: Vec<u8> = row.get("secret_ciphertext");
        let plaintext = decrypt(&self.key, id, kind, &ciphertext)?;
        let secret = CredentialSecret::decode(kind, &plaintext)?;

        sqlx::query("update credentials set last_used_at = now() where id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        Recorder::record_audit(
            &mut tx,
            actor_user_id,
            "worker",
            "credential.use",
            Some("credentials"),
            Some(id),
            "success",
            None,
        )
        .await?;
        tx.commit().await?;
        Ok(secret)
    }
}

fn scope_allows_device(scope: &CredentialScope, device_id: Uuid) -> bool {
    match scope {
        CredentialScope::Device {
            device_id: scoped_device_id,
        } => *scoped_device_id == device_id,
        // Group, integration, and network membership resolution is not part
        // of M6. They remain explicitly selected scopes; later management
        // modules can narrow these through their own membership resolvers.
        CredentialScope::DeviceGroup { .. }
        | CredentialScope::Integration { .. }
        | CredentialScope::Network { .. } => true,
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > MAX_NAME_BYTES || name.chars().any(char::is_control) {
        return Err(CredentialError::InvalidInput("credential name is invalid"));
    }
    Ok(())
}

fn aad(id: Uuid, kind: CredentialKind) -> Vec<u8> {
    let mut value = Vec::with_capacity(AAD_PREFIX.len() + 16 + 1);
    value.extend_from_slice(AAD_PREFIX);
    value.extend_from_slice(id.as_bytes());
    value.push(b':');
    value.extend_from_slice(kind.as_str().as_bytes());
    value
}

fn encrypt(key: &MasterKey, id: Uuid, kind: CredentialKind, plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new_from_slice(&*key.0)
        .map_err(|_| CredentialError::InvalidMasterKeyLength)?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            chacha20poly1305::aead::Payload {
                msg: plaintext,
                aad: &aad(id, kind),
            },
        )
        .map_err(|_| CredentialError::InvalidCiphertext)?;
    let mut encoded = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
    encoded.push(CIPHERTEXT_VERSION);
    encoded.extend_from_slice(&nonce_bytes);
    encoded.extend_from_slice(&ciphertext);
    Ok(encoded)
}

fn decrypt(
    key: &MasterKey,
    id: Uuid,
    kind: CredentialKind,
    encoded: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    if encoded.len() < 1 + NONCE_LEN + 16 || encoded[0] != CIPHERTEXT_VERSION {
        return Err(CredentialError::InvalidCiphertext);
    }
    let cipher = ChaCha20Poly1305::new_from_slice(&*key.0)
        .map_err(|_| CredentialError::InvalidMasterKeyLength)?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&encoded[1..1 + NONCE_LEN]),
            chacha20poly1305::aead::Payload {
                msg: &encoded[1 + NONCE_LEN..],
                aad: &aad(id, kind),
            },
        )
        .map_err(|_| CredentialError::InvalidCiphertext)?;
    if plaintext.len() > MAX_SECRET_BYTES {
        return Err(CredentialError::PayloadTooLarge);
    }
    Ok(Zeroizing::new(plaintext))
}

fn metadata_from_row(row: &sqlx::postgres::PgRow) -> Result<CredentialMetadata> {
    let scope: Value = row.try_get("scope")?;
    let scope = serde_json::from_value(scope)
        .map_err(|_| CredentialError::InvalidInput("stored credential scope is invalid"))?;
    Ok(CredentialMetadata {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        kind: CredentialKind::parse(row.try_get("kind")?)?,
        scope,
        version: row.try_get("version")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        last_used_at: row.try_get("last_used_at")?,
        revoked_at: row.try_get("revoked_at")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

/// Replace every supplied sensitive value before text is sent to logs or job
/// output. Empty values are ignored to avoid turning all text into markers.
pub fn redact_sensitive(input: &str, sensitive_values: &[&str]) -> String {
    sensitive_values
        .iter()
        .filter(|value| !value.is_empty())
        .fold(input.to_owned(), |redacted, value| {
            redacted.replace(value, "[REDACTED]")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> MasterKey {
        MasterKey::from_bytes([7; 32])
    }

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    #[test]
    fn accepts_hex_and_base64_keys() {
        let hex = MasterKey::from_encoded(&"ab".repeat(32)).unwrap();
        let base64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0xabu8; 32]);
        let encoded = MasterKey::from_encoded(&base64).unwrap();
        assert_eq!(format!("{hex:?}"), "MasterKey([REDACTED])");
        assert_eq!(format!("{encoded:?}"), "MasterKey([REDACTED])");
    }

    #[test]
    fn rejects_bad_key_lengths_and_encoding() {
        assert!(matches!(
            MasterKey::from_encoded("abc"),
            Err(CredentialError::InvalidMasterKeyEncoding | CredentialError::InvalidMasterKeyLength)
        ));
        assert!(matches!(
            MasterKey::from_encoded(&"ab".repeat(31)),
            Err(CredentialError::InvalidMasterKeyEncoding | CredentialError::InvalidMasterKeyLength)
        ));
    }

    #[test]
    fn encryption_round_trip_and_wrong_key_rejection() {
        let id = Uuid::new_v4();
        let kind = CredentialKind::SshPassword;
        let plaintext = br#"{"username":"root","password":"s3cret"}"#;
        let ciphertext = encrypt(&key(), id, kind, plaintext).unwrap();
        let decoded = decrypt(&key(), id, kind, &ciphertext).unwrap();
        assert_eq!(&*decoded, plaintext);
        assert!(matches!(
            decrypt(&MasterKey::from_bytes([8; 32]), id, kind, &ciphertext),
            Err(CredentialError::InvalidCiphertext)
        ));
    }

    #[test]
    fn ciphertext_is_bound_to_record_and_kind() {
        let id = Uuid::new_v4();
        let ciphertext = encrypt(&key(), id, CredentialKind::SshPassword, b"payload").unwrap();
        assert!(
            decrypt(
                &key(),
                Uuid::new_v4(),
                CredentialKind::SshPassword,
                &ciphertext
            )
            .is_err()
        );
        assert!(decrypt(&key(), id, CredentialKind::SshPrivateKey, &ciphertext).is_err());
    }

    #[test]
    fn secret_debug_and_redaction_do_not_leak() {
        let secret = CredentialSecret::SshPassword {
            username: Zeroizing::new("root".to_string()),
            password: Zeroizing::new("super-secret".to_string()),
        };
        assert!(!format!("{secret:?}").contains("super-secret"));
        let line = redact_sensitive(
            "ssh password=super-secret auth=Bearer super-secret",
            &["super-secret"],
        );
        assert!(!line.contains("super-secret"));
        assert_eq!(line, "ssh password=[REDACTED] auth=Bearer [REDACTED]");
    }

    #[test]
    fn validates_scopes_and_secret_kind() {
        let device_id = Uuid::new_v4();
        let secret = CredentialSecret::SshPassword {
            username: Zeroizing::new("root".into()),
            password: Zeroizing::new("pw".into()),
        };
        assert_eq!(secret.kind(), CredentialKind::SshPassword);
        assert!(CredentialScope::Device { device_id }.validate().is_ok());
        assert!(scope_allows_device(
            &CredentialScope::Device { device_id },
            device_id
        ));
        assert!(!scope_allows_device(
            &CredentialScope::Device {
                device_id: Uuid::new_v4()
            },
            device_id
        ));
        assert!(
            CredentialSecret::SshPassword {
                username: Zeroizing::new(String::new()),
                password: Zeroizing::new("pw".into())
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn secret_serialization_round_trips_without_debug_leakage() {
        let secret = CredentialSecret::SshPrivateKey {
            username: Zeroizing::new("root".into()),
            private_key_pem: Zeroizing::new("PRIVATE KEY MATERIAL".into()),
            passphrase: Some(Zeroizing::new("passphrase".into())),
        };
        let encoded = secret.encode().unwrap();
        let decoded = CredentialSecret::decode(CredentialKind::SshPrivateKey, &encoded).unwrap();
        assert_eq!(format!("{decoded:?}"), "CredentialSecret([REDACTED])");
        assert_eq!(encoded.len(), secret.encode().unwrap().len());
    }

    #[tokio::test]
    async fn database_round_trip_requires_active_device_association() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let device_id: Uuid = sqlx::query_scalar(
            "insert into devices (device_type, name) values ('physical_host', $1) returning id",
        )
        .bind(format!("credential-test-{}", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .unwrap();
        let store = CredentialStore::new(pool.clone(), key());
        let metadata = store
            .create(NewCredential {
                name: format!("credential-test-{device_id}"),
                scope: CredentialScope::Device { device_id },
                secret: CredentialSecret::SshPassword {
                    username: Zeroizing::new("root".into()),
                    password: Zeroizing::new("db-secret-value".into()),
                },
                created_by: None,
            })
            .await
            .unwrap();

        let ciphertext: Vec<u8> =
            sqlx::query_scalar("select secret_ciphertext from credentials where id = $1")
                .bind(metadata.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            !ciphertext
                .windows(b"db-secret-value".len())
                .any(|window| window == b"db-secret-value")
        );
        assert!(
            !serde_json::to_string(&metadata)
                .unwrap()
                .contains("db-secret-value")
        );
        assert!(matches!(
            store
                .decrypt_for_agent_use(metadata.id, device_id, None)
                .await,
            Err(CredentialError::Unavailable)
        ));

        sqlx::query(
            "insert into credential_associations (credential_id, device_id, purpose) \
             values ($1, $2, 'agent_install')",
        )
        .bind(metadata.id)
        .bind(device_id)
        .execute(&pool)
        .await
        .unwrap();
        let secret = store
            .decrypt_for_agent_use(metadata.id, device_id, None)
            .await
            .unwrap();
        assert!(matches!(
            secret,
            CredentialSecret::SshPassword { password, .. } if password.as_str() == "db-secret-value"
        ));
        assert!(
            store
                .disassociate_device(metadata.id, device_id, None)
                .await
                .unwrap()
        );
        assert!(matches!(
            store
                .decrypt_for_agent_use(metadata.id, device_id, None)
                .await,
            Err(CredentialError::Unavailable)
        ));

        sqlx::query("delete from credential_associations where credential_id = $1")
            .bind(metadata.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("delete from credentials where id = $1")
            .bind(metadata.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("delete from devices where id = $1")
            .bind(device_id)
            .execute(&pool)
            .await
            .unwrap();
    }
}
