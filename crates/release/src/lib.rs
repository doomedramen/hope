//! Signed agent release manifests (spec §7.5/§7.7, ADR-0007-adjacent
//! signing story). Shared between `xtask` (which signs releases) and the
//! agent (which verifies them before self-updating).
//!
//! Each artifact has an Ed25519 signature over its canonical record encoding,
//! and the manifest file has a second, detached signature over its exact
//! bytes. Callers verifying a release bundle should use [`verify_manifest`]
//! or [`verify_manifest_and_binary`], then never consume an artifact record
//! that was not returned by that verification.

use std::{collections::BTreeSet, fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use sha2::{Digest, Sha256};

/// Maximum raw `manifest.json` size accepted by bundle verification.
pub const MAX_MANIFEST_BYTES: usize = 512 * 1024;
/// Maximum number of artifact records accepted in one manifest.
pub const MAX_MANIFEST_ARTIFACTS: usize = 16;
/// Maximum number of trusted Ed25519 keys accepted for rotation.
pub const MAX_TRUSTED_KEYS: usize = 8;
/// Maximum byte length for manifest string fields such as version/platform.
pub const MAX_MANIFEST_STRING_BYTES: usize = 128;
/// Maximum UTF-8 byte length for optional release notes.
pub const MAX_RELEASE_NOTES_BYTES: usize = 4096;
/// Maximum binary size accepted by artifact verification.
pub const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
/// SHA-256 encoded as lowercase hexadecimal.
pub const SHA256_HEX_BYTES: usize = 64;
/// Ed25519 signature encoded as hexadecimal.
pub const SIGNATURE_HEX_BYTES: usize = 128;

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("manifest is too large: maximum {max} bytes, got {actual}")]
    ManifestTooLarge { max: usize, actual: usize },
    #[error("manifest contains too many artifacts: maximum {max}, got {actual}")]
    TooManyArtifacts { max: usize, actual: usize },
    #[error("too many trusted release keys: maximum {max}, got {actual}")]
    TooManyTrustedKeys { max: usize, actual: usize },
    #[error("no trusted release keys configured")]
    NoTrustedKeys,
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("invalid manifest field {field}: {reason}")]
    InvalidField {
        field: &'static str,
        reason: &'static str,
    },
    #[error("release {manifest_version} does not match artifact version {artifact_version}")]
    ManifestVersionMismatch {
        manifest_version: String,
        artifact_version: String,
    },
    #[error("duplicate artifact for {platform}/{arch}")]
    DuplicateArtifact { platform: String, arch: String },
    #[error("artifact is too large: maximum {max} bytes, got {actual}")]
    ArtifactTooLarge { max: u64, actual: u64 },
    #[error("binary size mismatch: expected {expected}, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("binary sha256 mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("platform mismatch: expected {expected}, got {actual}")]
    PlatformMismatch { expected: String, actual: String },
    #[error("architecture mismatch: expected {expected}, got {actual}")]
    ArchMismatch { expected: String, actual: String },
    #[error("artifact requires protocol version {required}, but verifier supports {current}")]
    ProtocolVersionMismatch { required: u32, current: u32 },
    #[error("invalid signature encoding: {0}")]
    InvalidSignatureEncoding(String),
    #[error("signature verification failed")]
    InvalidSignature,
    #[error("json encoding failed: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("json decoding failed: {0}")]
    Decode(String),
}

pub type Result<T> = std::result::Result<T, ReleaseError>;

/// Everything about one platform/arch build of the agent that the signature
/// covers. Field order here is the canonical wire order — do not reorder
/// without treating it as a breaking change to already-issued signatures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub version: String,
    pub platform: String,
    pub arch: String,
    pub size: u64,
    /// Lowercase hex-encoded SHA-256 digest of the binary.
    pub sha256: String,
    pub min_protocol_version: u32,
}

impl ArtifactRecord {
    /// Deterministic byte encoding that both signer and verifier hash /
    /// sign over. `serde_json` preserves struct field declaration order
    /// (this isn't a `HashMap`), so this is stable across runs as long as
    /// the struct definition doesn't change.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        // Safe to unwrap: `ArtifactRecord` contains only strings/integers,
        // which always serialize successfully.
        serde_json::to_vec(self).expect("ArtifactRecord is always serializable")
    }

    pub fn sha256_of(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex::encode(hasher.finalize())
    }

    /// Validate fields before using them as release metadata.
    pub fn validate(&self) -> Result<()> {
        validate_version("artifact.version", &self.version)?;
        validate_string("artifact.platform", &self.platform)?;
        validate_string("artifact.arch", &self.arch)?;

        if self.size > MAX_ARTIFACT_BYTES {
            return Err(ReleaseError::ArtifactTooLarge {
                max: MAX_ARTIFACT_BYTES,
                actual: self.size,
            });
        }

        if self.sha256.len() != SHA256_HEX_BYTES
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ReleaseError::InvalidField {
                field: "artifact.sha256",
                reason: "expected 64 lowercase hexadecimal bytes",
            });
        }

        Ok(())
    }
}

/// A record plus its detached Ed25519 signature (lowercase hex), as
/// carried in `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedArtifact {
    #[serde(flatten)]
    pub record: ArtifactRecord,
    pub signature: String,
}

/// Release channel encoded in signed manifest metadata.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseChannel {
    #[default]
    Stable,
    Canary,
}

impl fmt::Display for ReleaseChannel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stable => formatter.write_str("stable"),
            Self::Canary => formatter.write_str("canary"),
        }
    }
}

impl FromStr for ReleaseChannel {
    type Err = &'static str;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "stable" => Ok(Self::Stable),
            "canary" => Ok(Self::Canary),
            _ => Err("expected stable or canary"),
        }
    }
}

/// Metadata that applies to one complete release bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestMetadata {
    #[serde(default)]
    pub channel: ReleaseChannel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_notes: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestMetadataFields {
    #[serde(default)]
    channel: ReleaseChannel,
    #[serde(default)]
    release_notes: Option<String>,
}

impl<'de> Deserialize<'de> for ManifestMetadata {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let fields = ManifestMetadataFields::deserialize(deserializer)?;
        let metadata = Self {
            channel: fields.channel,
            release_notes: fields.release_notes,
        };
        metadata.validate().map_err(D::Error::custom)?;
        Ok(metadata)
    }
}

impl Default for ManifestMetadata {
    fn default() -> Self {
        Self {
            channel: ReleaseChannel::Stable,
            release_notes: None,
        }
    }
}

impl ManifestMetadata {
    /// Validate metadata bounds before it is serialized or consumed.
    pub fn validate(&self) -> Result<()> {
        if let Some(release_notes) = &self.release_notes
            && release_notes.len() > MAX_RELEASE_NOTES_BYTES
        {
            return Err(ReleaseError::InvalidField {
                field: "manifest.release_notes",
                reason: "exceeds the release notes size limit",
            });
        }
        Ok(())
    }
}

/// The full manifest written alongside a release: one entry per
/// platform/arch, plus the overall release version they belong to.
///
/// This legacy view stays source-compatible with callers that construct a
/// manifest directly. Use [`ManifestWithMetadata`] for newly signed bundles.
#[derive(Debug, Clone, Serialize)]
pub struct Manifest {
    pub version: String,
    pub artifacts: Vec<SignedArtifact>,
}

impl Manifest {
    /// Add signed release metadata without changing the legacy manifest
    /// construction API used by older agent code.
    pub fn with_metadata(self, metadata: ManifestMetadata) -> ManifestWithMetadata {
        ManifestWithMetadata {
            version: self.version,
            channel: metadata.channel,
            release_notes: metadata.release_notes,
            artifacts: self.artifacts,
        }
    }
}

/// Complete manifest wire format, including channel and optional release
/// notes. Missing metadata in older JSON manifests means stable with no notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestWithMetadata {
    pub version: String,
    pub channel: ReleaseChannel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_notes: Option<String>,
    pub artifacts: Vec<SignedArtifact>,
}

impl ManifestWithMetadata {
    /// Return metadata as a standalone value for callers that do not need
    /// artifact records.
    pub fn metadata(&self) -> ManifestMetadata {
        ManifestMetadata {
            channel: self.channel,
            release_notes: self.release_notes.clone(),
        }
    }

    /// Validate metadata and all legacy manifest invariants.
    pub fn validate(&self) -> Result<()> {
        self.metadata().validate()?;
        Manifest {
            version: self.version.clone(),
            artifacts: self.artifacts.clone(),
        }
        .validate()
    }

    /// Return the source-compatible manifest view, dropping metadata only
    /// from the returned Rust value. Metadata remains covered by its
    /// detached signature on the wire.
    pub fn into_manifest(self) -> Manifest {
        Manifest {
            version: self.version,
            artifacts: self.artifacts,
        }
    }
}

impl From<Manifest> for ManifestWithMetadata {
    fn from(manifest: Manifest) -> Self {
        manifest.with_metadata(ManifestMetadata::default())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFields {
    version: String,
    #[serde(default)]
    channel: ReleaseChannel,
    #[serde(default)]
    release_notes: Option<String>,
    artifacts: Vec<SignedArtifact>,
}

impl<'de> Deserialize<'de> for Manifest {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let fields = ManifestFields::deserialize(deserializer)?;
        let manifest = Self {
            version: fields.version,
            artifacts: fields.artifacts,
        };
        ManifestMetadata {
            channel: fields.channel,
            release_notes: fields.release_notes,
        }
        .validate()
        .map_err(D::Error::custom)?;
        manifest.validate().map_err(D::Error::custom)?;
        Ok(manifest)
    }
}

impl<'de> Deserialize<'de> for ManifestWithMetadata {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let fields = ManifestFields::deserialize(deserializer)?;
        let manifest = Self {
            version: fields.version,
            channel: fields.channel,
            release_notes: fields.release_notes,
            artifacts: fields.artifacts,
        };
        manifest.validate().map_err(D::Error::custom)?;
        Ok(manifest)
    }
}

fn validate_string(field: &'static str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(ReleaseError::InvalidField {
            field,
            reason: "must not be empty",
        });
    }
    if value.len() > MAX_MANIFEST_STRING_BYTES {
        return Err(ReleaseError::InvalidField {
            field,
            reason: "exceeds the release metadata size limit",
        });
    }
    Ok(())
}

fn validate_version(field: &'static str, value: &str) -> Result<()> {
    validate_string(field, value)?;
    let (core, prerelease) = value.split_once('-').unwrap_or((value, ""));
    let components: Vec<_> = core.split('.').collect();
    if components.len() != 3
        || components.iter().any(|component| {
            component.is_empty()
                || (component.len() > 1 && component.starts_with('0'))
                || !component.bytes().all(|byte| byte.is_ascii_digit())
        })
        || (!prerelease.is_empty()
            && prerelease.split('.').any(|component| {
                component.is_empty()
                    || !component
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            }))
    {
        return Err(ReleaseError::InvalidField {
            field,
            reason: "expected semantic version MAJOR.MINOR.PATCH[-PRERELEASE]",
        });
    }
    Ok(())
}

impl SignedArtifact {
    fn validate(&self) -> Result<()> {
        self.record.validate()?;
        decode_signature(&self.signature)?;
        Ok(())
    }
}

impl Manifest {
    /// Validate manifest-wide bounds and invariants independent of signatures.
    pub fn validate(&self) -> Result<()> {
        validate_version("manifest.version", &self.version)?;
        if self.artifacts.is_empty() {
            return Err(ReleaseError::InvalidManifest(
                "manifest must contain at least one artifact".to_string(),
            ));
        }
        if self.artifacts.len() > MAX_MANIFEST_ARTIFACTS {
            return Err(ReleaseError::TooManyArtifacts {
                max: MAX_MANIFEST_ARTIFACTS,
                actual: self.artifacts.len(),
            });
        }

        let mut platforms_and_arches = BTreeSet::new();
        for artifact in &self.artifacts {
            artifact.validate()?;
            if artifact.record.version != self.version {
                return Err(ReleaseError::ManifestVersionMismatch {
                    manifest_version: self.version.clone(),
                    artifact_version: artifact.record.version.clone(),
                });
            }
            let identity = (&artifact.record.platform, &artifact.record.arch);
            if !platforms_and_arches.insert(identity) {
                return Err(ReleaseError::DuplicateArtifact {
                    platform: artifact.record.platform.clone(),
                    arch: artifact.record.arch.clone(),
                });
            }
        }

        Ok(())
    }
}

// --- signing (used by xtask; kept here so the exact byte encoding signed
// and verified can never drift between the two sides) -------------------

pub fn sign_record(record: &ArtifactRecord, signing_key: &ed25519_dalek::SigningKey) -> String {
    use ed25519_dalek::Signer;
    let signature = signing_key.sign(&record.canonical_bytes());
    hex::encode(signature.to_bytes())
}

/// Sign the exact bytes written to `manifest.json` for detached verification.
pub fn sign_manifest(manifest_bytes: &[u8], signing_key: &ed25519_dalek::SigningKey) -> String {
    use ed25519_dalek::Signer;
    let signature = signing_key.sign(manifest_bytes);
    hex::encode(signature.to_bytes())
}

// --- verification (used by both xtask, for self-checking what it just
// signed, and eventually the agent) --------------------------------------

fn decode_signature(signature_hex: &str) -> Result<ed25519_dalek::Signature> {
    if signature_hex.len() > SIGNATURE_HEX_BYTES {
        return Err(ReleaseError::InvalidSignatureEncoding(format!(
            "signature exceeds {SIGNATURE_HEX_BYTES} hexadecimal bytes"
        )));
    }
    let bytes = hex::decode(signature_hex)
        .map_err(|err| ReleaseError::InvalidSignatureEncoding(err.to_string()))?;
    let bytes: [u8; 64] = bytes
        .try_into()
        .map_err(|_| ReleaseError::InvalidSignatureEncoding("expected 64 bytes".to_string()))?;
    Ok(ed25519_dalek::Signature::from_bytes(&bytes))
}

fn verify_signed_bytes(
    message: &[u8],
    signature_hex: &str,
    trusted_keys: &[ed25519_dalek::VerifyingKey],
) -> Result<()> {
    if trusted_keys.is_empty() {
        return Err(ReleaseError::NoTrustedKeys);
    }
    if trusted_keys.len() > MAX_TRUSTED_KEYS {
        return Err(ReleaseError::TooManyTrustedKeys {
            max: MAX_TRUSTED_KEYS,
            actual: trusted_keys.len(),
        });
    }

    use ed25519_dalek::Verifier;
    let signature = decode_signature(signature_hex)?;
    if trusted_keys
        .iter()
        .any(|key| key.verify(message, &signature).is_ok())
    {
        Ok(())
    } else {
        Err(ReleaseError::InvalidSignature)
    }
}

/// Verify that `signature_hex` is a valid Ed25519 signature by
/// `verifying_key` over `record`'s canonical bytes. Does not touch any
/// binary or platform/arch expectations — see [`verify_binary`] for that.
pub fn verify_record_signature(
    record: &ArtifactRecord,
    signature_hex: &str,
    verifying_key: &ed25519_dalek::VerifyingKey,
) -> Result<()> {
    verify_record_signature_with_keys(record, signature_hex, std::slice::from_ref(verifying_key))
}

/// Verify an artifact record against any one key in a bounded rotation set.
pub fn verify_record_signature_with_keys(
    record: &ArtifactRecord,
    signature_hex: &str,
    trusted_keys: &[ed25519_dalek::VerifyingKey],
) -> Result<()> {
    record.validate()?;
    verify_signed_bytes(&record.canonical_bytes(), signature_hex, trusted_keys)
}

/// Verify a detached signature over the exact raw manifest bytes.
pub fn verify_manifest_signature(
    manifest_bytes: &[u8],
    signature_hex: &str,
    trusted_keys: &[ed25519_dalek::VerifyingKey],
) -> Result<()> {
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ReleaseError::ManifestTooLarge {
            max: MAX_MANIFEST_BYTES,
            actual: manifest_bytes.len(),
        });
    }
    verify_signed_bytes(manifest_bytes, signature_hex, trusted_keys)
}

/// Verify a detached manifest signature, parse the manifest, and validate all
/// manifest-wide consistency, metadata, and resource constraints.
pub fn verify_manifest_with_metadata(
    manifest_bytes: &[u8],
    manifest_signature_hex: &str,
    trusted_keys: &[ed25519_dalek::VerifyingKey],
) -> Result<ManifestWithMetadata> {
    verify_manifest_signature(manifest_bytes, manifest_signature_hex, trusted_keys)?;
    let fields: ManifestFields = serde_json::from_slice(manifest_bytes)
        .map_err(|err| ReleaseError::Decode(err.to_string()))?;
    let manifest = ManifestWithMetadata {
        version: fields.version,
        channel: fields.channel,
        release_notes: fields.release_notes,
        artifacts: fields.artifacts,
    };
    manifest.validate()?;
    for artifact in &manifest.artifacts {
        verify_record_signature_with_keys(&artifact.record, &artifact.signature, trusted_keys)?;
    }
    Ok(manifest)
}

/// Verify a detached manifest signature, parse the manifest, and validate all
/// manifest-wide consistency and resource constraints. Older callers receive
/// the source-compatible view without the new metadata fields.
pub fn verify_manifest(
    manifest_bytes: &[u8],
    manifest_signature_hex: &str,
    trusted_keys: &[ed25519_dalek::VerifyingKey],
) -> Result<Manifest> {
    Ok(
        verify_manifest_with_metadata(manifest_bytes, manifest_signature_hex, trusted_keys)?
            .into_manifest(),
    )
}

/// Verify a downloaded artifact with the legacy single-key API. This keeps
/// `xtask` and the current CLI caller source-compatible. Use
/// [`verify_binary_with_protocol`] or [`verify_binary_with_keys`] when the
/// verifier also has a current protocol version and/or a rotation set.
pub fn verify_binary(
    record: &ArtifactRecord,
    signature_hex: &str,
    verifying_key: &ed25519_dalek::VerifyingKey,
    binary_bytes: &[u8],
    expected_platform: &str,
    expected_arch: &str,
) -> Result<()> {
    verify_binary_inner(
        record,
        signature_hex,
        std::slice::from_ref(verifying_key),
        binary_bytes,
        expected_platform,
        expected_arch,
        None,
    )
}

/// Full artifact verification using a bounded trusted-key rotation set and a
/// caller-supplied protocol version.
pub fn verify_binary_with_keys(
    record: &ArtifactRecord,
    signature_hex: &str,
    trusted_keys: &[ed25519_dalek::VerifyingKey],
    binary_bytes: &[u8],
    expected_platform: &str,
    expected_arch: &str,
    current_protocol_version: u32,
) -> Result<()> {
    verify_binary_inner(
        record,
        signature_hex,
        trusted_keys,
        binary_bytes,
        expected_platform,
        expected_arch,
        Some(current_protocol_version),
    )
}

/// Single-key convenience wrapper that also enforces minimum protocol version.
pub fn verify_binary_with_protocol(
    record: &ArtifactRecord,
    signature_hex: &str,
    verifying_key: &ed25519_dalek::VerifyingKey,
    binary_bytes: &[u8],
    expected_platform: &str,
    expected_arch: &str,
    current_protocol_version: u32,
) -> Result<()> {
    verify_binary_with_keys(
        record,
        signature_hex,
        std::slice::from_ref(verifying_key),
        binary_bytes,
        expected_platform,
        expected_arch,
        current_protocol_version,
    )
}

fn verify_binary_inner(
    record: &ArtifactRecord,
    signature_hex: &str,
    trusted_keys: &[ed25519_dalek::VerifyingKey],
    binary_bytes: &[u8],
    expected_platform: &str,
    expected_arch: &str,
    current_protocol_version: Option<u32>,
) -> Result<()> {
    record.validate()?;
    verify_signed_bytes(&record.canonical_bytes(), signature_hex, trusted_keys)?;

    if record.platform != expected_platform {
        return Err(ReleaseError::PlatformMismatch {
            expected: expected_platform.to_string(),
            actual: record.platform.clone(),
        });
    }
    if record.arch != expected_arch {
        return Err(ReleaseError::ArchMismatch {
            expected: expected_arch.to_string(),
            actual: record.arch.clone(),
        });
    }

    if let Some(current_protocol_version) = current_protocol_version
        && record.min_protocol_version > current_protocol_version
    {
        return Err(ReleaseError::ProtocolVersionMismatch {
            required: record.min_protocol_version,
            current: current_protocol_version,
        });
    }

    let actual_size = binary_bytes.len() as u64;
    if actual_size > MAX_ARTIFACT_BYTES {
        return Err(ReleaseError::ArtifactTooLarge {
            max: MAX_ARTIFACT_BYTES,
            actual: actual_size,
        });
    }

    if record.size != actual_size {
        return Err(ReleaseError::SizeMismatch {
            expected: record.size,
            actual: actual_size,
        });
    }

    let actual_sha256 = ArtifactRecord::sha256_of(binary_bytes);
    if record.sha256 != actual_sha256 {
        return Err(ReleaseError::HashMismatch {
            expected: record.sha256.clone(),
            actual: actual_sha256,
        });
    }

    Ok(())
}

/// Verify a complete manifest and the artifact matching the expected target.
/// Returns the verified manifest entry so callers cannot accidentally select a
/// different, unverified record after this function returns.
pub fn verify_manifest_and_binary(
    manifest_bytes: &[u8],
    manifest_signature_hex: &str,
    trusted_keys: &[ed25519_dalek::VerifyingKey],
    binary_bytes: &[u8],
    expected_platform: &str,
    expected_arch: &str,
    current_protocol_version: u32,
) -> Result<SignedArtifact> {
    Ok(verify_manifest_and_binary_with_metadata(
        manifest_bytes,
        manifest_signature_hex,
        trusted_keys,
        binary_bytes,
        expected_platform,
        expected_arch,
        current_protocol_version,
    )?
    .1)
}

/// Verify a complete manifest, including metadata, and the artifact matching
/// the expected target. Returns both verified values so callers can apply
/// channel or release-note policy without reparsing signed bytes.
pub fn verify_manifest_and_binary_with_metadata(
    manifest_bytes: &[u8],
    manifest_signature_hex: &str,
    trusted_keys: &[ed25519_dalek::VerifyingKey],
    binary_bytes: &[u8],
    expected_platform: &str,
    expected_arch: &str,
    current_protocol_version: u32,
) -> Result<(ManifestWithMetadata, SignedArtifact)> {
    let manifest =
        verify_manifest_with_metadata(manifest_bytes, manifest_signature_hex, trusted_keys)?;
    let artifact = {
        let artifact = manifest
            .artifacts
            .iter()
            .find(|artifact| {
                artifact.record.platform == expected_platform
                    && artifact.record.arch == expected_arch
            })
            .ok_or_else(|| {
                ReleaseError::InvalidManifest(format!(
                    "no artifact for {expected_platform}/{expected_arch}"
                ))
            })?;

        verify_binary_with_keys(
            &artifact.record,
            &artifact.signature,
            trusted_keys,
            binary_bytes,
            expected_platform,
            expected_arch,
            current_protocol_version,
        )?;
        artifact.clone()
    };

    Ok((manifest, artifact))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand_core::OsRng;

    fn keypair() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    fn sample_record() -> ArtifactRecord {
        let binary = b"pretend this is an agent binary";
        ArtifactRecord {
            version: "0.1.0".to_string(),
            platform: "linux".to_string(),
            arch: "amd64".to_string(),
            size: binary.len() as u64,
            sha256: ArtifactRecord::sha256_of(binary),
            min_protocol_version: 1,
        }
    }

    fn sign(record: &ArtifactRecord, key: &SigningKey) -> String {
        sign_record(record, key)
    }

    fn manifest_json(record: ArtifactRecord, signature: String) -> Vec<u8> {
        serde_json::to_vec_pretty(&Manifest {
            version: record.version.clone(),
            artifacts: vec![SignedArtifact { record, signature }],
        })
        .expect("sample manifest should serialize")
    }

    fn manifest_with_metadata_json(
        record: ArtifactRecord,
        signature: String,
        channel: ReleaseChannel,
        release_notes: Option<String>,
    ) -> Vec<u8> {
        serde_json::to_vec_pretty(
            &Manifest {
                version: record.version.clone(),
                artifacts: vec![SignedArtifact { record, signature }],
            }
            .with_metadata(ManifestMetadata {
                channel,
                release_notes,
            }),
        )
        .expect("sample manifest with metadata should serialize")
    }

    fn sign_manifest_bytes(manifest_bytes: &[u8], key: &SigningKey) -> String {
        sign_manifest(manifest_bytes, key)
    }

    #[test]
    fn valid_signature_and_binary_verify() {
        let key = keypair();
        let record = sample_record();
        let signature = sign(&record, &key);
        let binary = b"pretend this is an agent binary";

        verify_binary(
            &record,
            &signature,
            &key.verifying_key(),
            binary,
            "linux",
            "amd64",
        )
        .expect("valid artifact should verify");
    }

    #[test]
    fn tampered_binary_is_rejected() {
        let key = keypair();
        let record = sample_record();
        let signature = sign(&record, &key);
        // Same length as the signed binary (32 bytes) so this specifically
        // exercises the hash check, not the (also-checked) size check.
        let tampered = b"pretend THIS is an agent binary";
        assert_eq!(tampered.len(), b"pretend this is an agent binary".len());

        let err = verify_binary(
            &record,
            &signature,
            &key.verifying_key(),
            tampered,
            "linux",
            "amd64",
        )
        .unwrap_err();
        assert!(matches!(err, ReleaseError::HashMismatch { .. }));
    }

    #[test]
    fn wrong_key_is_rejected() {
        let key = keypair();
        let wrong_key = keypair();
        let record = sample_record();
        let signature = sign(&record, &key);
        let binary = b"pretend this is an agent binary";

        let err = verify_binary(
            &record,
            &signature,
            &wrong_key.verifying_key(),
            binary,
            "linux",
            "amd64",
        )
        .unwrap_err();
        assert!(matches!(err, ReleaseError::InvalidSignature));
    }

    #[test]
    fn trusted_key_rotation_accepts_new_key() {
        let old_key = keypair();
        let new_key = keypair();
        let record = sample_record();
        let signature = sign(&record, &new_key);
        let binary = b"pretend this is an agent binary";
        let trusted_keys = vec![old_key.verifying_key(), new_key.verifying_key()];

        verify_binary_with_keys(
            &record,
            &signature,
            &trusted_keys,
            binary,
            "linux",
            "amd64",
            1,
        )
        .expect("new rotation key should be trusted during overlap");
    }

    #[test]
    fn tampered_manifest_record_is_rejected() {
        let key = keypair();
        let record = sample_record();
        let signature = sign(&record, &key);
        let binary = b"pretend this is an agent binary";

        // Attacker edits the manifest after signing (e.g. bumps the
        // min_protocol_version to bypass a compatibility gate, or claims
        // a different arch) without re-signing.
        let mut tampered_record = record.clone();
        tampered_record.min_protocol_version = 999;

        let err = verify_binary(
            &tampered_record,
            &signature,
            &key.verifying_key(),
            binary,
            "linux",
            "amd64",
        )
        .unwrap_err();
        assert!(matches!(err, ReleaseError::InvalidSignature));
    }

    #[test]
    fn platform_and_arch_mismatch_are_rejected() {
        let key = keypair();
        let record = sample_record();
        let signature = sign(&record, &key);
        let binary = b"pretend this is an agent binary";

        let err = verify_binary(
            &record,
            &signature,
            &key.verifying_key(),
            binary,
            "windows",
            "amd64",
        )
        .unwrap_err();
        assert!(matches!(err, ReleaseError::PlatformMismatch { .. }));

        let err = verify_binary(
            &record,
            &signature,
            &key.verifying_key(),
            binary,
            "linux",
            "arm64",
        )
        .unwrap_err();
        assert!(matches!(err, ReleaseError::ArchMismatch { .. }));
    }

    #[test]
    fn minimum_protocol_version_is_enforced() {
        let key = keypair();
        let mut record = sample_record();
        record.min_protocol_version = 2;
        let signature = sign(&record, &key);
        let binary = b"pretend this is an agent binary";

        let err = verify_binary_with_protocol(
            &record,
            &signature,
            &key.verifying_key(),
            binary,
            "linux",
            "amd64",
            1,
        )
        .unwrap_err();
        assert!(matches!(err, ReleaseError::ProtocolVersionMismatch { .. }));
    }

    #[test]
    fn detached_manifest_signature_covers_tampering() {
        let key = keypair();
        let record = sample_record();
        let artifact_signature = sign(&record, &key);
        let manifest = manifest_json(record, artifact_signature);
        let manifest_signature = sign_manifest_bytes(&manifest, &key);
        let mut tampered = manifest.clone();
        tampered[0] = b' ';

        let err = verify_manifest(&tampered, &manifest_signature, &[key.verifying_key()])
            .expect_err("tampered manifest must fail detached signature");
        assert!(matches!(err, ReleaseError::InvalidSignature));
    }

    #[test]
    fn unsigned_or_tampered_artifact_is_rejected() {
        let key = keypair();
        let record = sample_record();
        let manifest = Manifest {
            version: record.version.clone(),
            artifacts: vec![SignedArtifact {
                record: record.clone(),
                signature: String::new(),
            }],
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("manifest should encode");
        let manifest_signature = sign_manifest_bytes(&manifest_bytes, &key);

        let err = verify_manifest_and_binary(
            &manifest_bytes,
            &manifest_signature,
            &[key.verifying_key()],
            b"pretend this is an agent binary",
            "linux",
            "amd64",
            1,
        )
        .expect_err("unsigned artifact must fail");
        assert!(matches!(err, ReleaseError::InvalidSignatureEncoding(_)));

        let artifact_signature = sign(&record, &key);
        let manifest = manifest_json(record, artifact_signature);
        let mut tampered_manifest: serde_json::Value =
            serde_json::from_slice(&manifest).expect("manifest should parse");
        tampered_manifest["artifacts"][0]["sha256"] = serde_json::Value::String(
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        );
        let tampered_manifest =
            serde_json::to_vec_pretty(&tampered_manifest).expect("tampered manifest should encode");
        let tampered_manifest_signature = sign_manifest_bytes(&tampered_manifest, &key);
        verify_manifest_signature(
            &tampered_manifest,
            &tampered_manifest_signature,
            &[key.verifying_key()],
        )
        .expect("tampered artifact manifest should retain detached signature");
        let err = verify_manifest_and_binary(
            &tampered_manifest,
            &tampered_manifest_signature,
            &[key.verifying_key()],
            b"pretend this is an agent binary",
            "linux",
            "amd64",
            1,
        )
        .expect_err("tampered artifact record must fail its own signature");
        assert!(matches!(err, ReleaseError::InvalidSignature));
    }

    #[test]
    fn manifest_rejects_version_mismatch_and_duplicate_target() {
        let key = keypair();
        let record = sample_record();
        let artifact_signature = sign(&record, &key);
        let mut manifest = Manifest {
            version: "0.2.0".to_string(),
            artifacts: vec![SignedArtifact {
                record: record.clone(),
                signature: artifact_signature.clone(),
            }],
        };
        let bytes = serde_json::to_vec(&manifest).expect("manifest should encode");
        let signature = sign_manifest_bytes(&bytes, &key);
        let err = verify_manifest(&bytes, &signature, &[key.verifying_key()]).unwrap_err();
        assert!(matches!(err, ReleaseError::ManifestVersionMismatch { .. }));

        manifest.version = record.version.clone();
        manifest.artifacts.push(SignedArtifact {
            record,
            signature: artifact_signature,
        });
        let bytes = serde_json::to_vec(&manifest).expect("manifest should encode");
        let signature = sign_manifest_bytes(&bytes, &key);
        let err = verify_manifest(&bytes, &signature, &[key.verifying_key()]).unwrap_err();
        assert!(matches!(err, ReleaseError::DuplicateArtifact { .. }));
    }

    #[test]
    fn resource_bounds_reject_large_manifest_and_key_list() {
        let key = keypair();
        let oversized_manifest = vec![b' '; MAX_MANIFEST_BYTES + 1];
        let err = verify_manifest_signature(
            &oversized_manifest,
            &sign_manifest_bytes(&oversized_manifest, &key),
            &[key.verifying_key()],
        )
        .unwrap_err();
        assert!(matches!(err, ReleaseError::ManifestTooLarge { .. }));

        let trusted_keys = vec![key.verifying_key(); MAX_TRUSTED_KEYS + 1];
        let record = sample_record();
        let err = verify_record_signature_with_keys(&record, &sign(&record, &key), &trusted_keys)
            .unwrap_err();
        assert!(matches!(err, ReleaseError::TooManyTrustedKeys { .. }));

        let mut oversized_record = sample_record();
        oversized_record.size = MAX_ARTIFACT_BYTES + 1;
        let err = oversized_record.validate().unwrap_err();
        assert!(matches!(err, ReleaseError::ArtifactTooLarge { .. }));

        let artifact_signature = sign(&sample_record(), &key);
        let oversized_manifest = Manifest {
            version: "0.1.0".to_string(),
            artifacts: vec![
                SignedArtifact {
                    record: sample_record(),
                    signature: artifact_signature,
                };
                MAX_MANIFEST_ARTIFACTS + 1
            ],
        };
        let manifest_bytes =
            serde_json::to_vec(&oversized_manifest).expect("manifest should encode");
        let manifest_signature = sign_manifest_bytes(&manifest_bytes, &key);
        let err = verify_manifest(&manifest_bytes, &manifest_signature, &[key.verifying_key()])
            .unwrap_err();
        assert!(matches!(err, ReleaseError::TooManyArtifacts { .. }));
    }

    #[test]
    fn manifest_json_roundtrips() {
        let key = keypair();
        let record = sample_record();
        let signature = sign(&record, &key);
        let manifest = Manifest {
            version: "0.1.0".to_string(),
            artifacts: vec![SignedArtifact { record, signature }],
        };

        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let decoded: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.artifacts.len(), 1);
        assert_eq!(decoded.artifacts[0].record.version, "0.1.0");
    }

    #[test]
    fn legacy_manifest_defaults_to_stable_without_release_notes() {
        let key = keypair();
        let record = sample_record();
        let artifact_signature = sign(&record, &key);
        let manifest = manifest_json(record, artifact_signature);
        let manifest_signature = sign_manifest_bytes(&manifest, &key);

        let decoded =
            verify_manifest_with_metadata(&manifest, &manifest_signature, &[key.verifying_key()])
                .expect("legacy manifest should remain valid");
        assert_eq!(decoded.channel, ReleaseChannel::Stable);
        assert_eq!(decoded.release_notes, None);
    }

    #[test]
    fn signed_channel_and_release_notes_roundtrip() {
        let key = keypair();
        let record = sample_record();
        let artifact_signature = sign(&record, &key);
        let manifest = manifest_with_metadata_json(
            record,
            artifact_signature,
            ReleaseChannel::Canary,
            Some("Test canary before stable rollout.".to_string()),
        );
        let manifest_signature = sign_manifest_bytes(&manifest, &key);

        let decoded =
            verify_manifest_with_metadata(&manifest, &manifest_signature, &[key.verifying_key()])
                .expect("signed metadata should verify");
        assert_eq!(decoded.channel, ReleaseChannel::Canary);
        assert_eq!(
            decoded.release_notes.as_deref(),
            Some("Test canary before stable rollout.")
        );

        let mut tampered: serde_json::Value =
            serde_json::from_slice(&manifest).expect("manifest should parse");
        tampered["channel"] = serde_json::Value::String("stable".to_string());
        let tampered =
            serde_json::to_vec_pretty(&tampered).expect("tampered manifest should encode");
        let err =
            verify_manifest_with_metadata(&tampered, &manifest_signature, &[key.verifying_key()])
                .expect_err("metadata tampering must fail detached signature");
        assert!(matches!(err, ReleaseError::InvalidSignature));
    }

    #[test]
    fn release_notes_are_bounded() {
        let record = sample_record();
        let manifest = Manifest {
            version: record.version.clone(),
            artifacts: vec![SignedArtifact {
                record,
                signature: "0".repeat(SIGNATURE_HEX_BYTES),
            }],
        }
        .with_metadata(ManifestMetadata {
            channel: ReleaseChannel::Stable,
            release_notes: Some("x".repeat(MAX_RELEASE_NOTES_BYTES + 1)),
        });

        assert!(matches!(
            manifest.validate(),
            Err(ReleaseError::InvalidField {
                field: "manifest.release_notes",
                ..
            })
        ));
    }

    #[test]
    fn release_channel_parser_rejects_unknown_values() {
        assert_eq!(
            "stable".parse::<ReleaseChannel>(),
            Ok(ReleaseChannel::Stable)
        );
        assert_eq!(
            "canary".parse::<ReleaseChannel>(),
            Ok(ReleaseChannel::Canary)
        );
        assert!("beta".parse::<ReleaseChannel>().is_err());
    }

    #[test]
    fn semantic_versions_are_required() {
        let mut record = sample_record();
        record.version = "release-latest".to_string();
        assert!(matches!(
            record.validate(),
            Err(ReleaseError::InvalidField {
                field: "artifact.version",
                ..
            })
        ));
    }
}
