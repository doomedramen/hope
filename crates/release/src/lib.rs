//! Signed agent release manifests (spec §7.5/§7.7, ADR-0007-adjacent
//! signing story). Shared between `xtask` (which signs releases) and the
//! agent (which will verify them before self-updating).
//!
//! An [`ArtifactRecord`] describes one platform/arch binary. It is signed
//! with Ed25519 over its own canonical byte encoding (deterministic
//! `serde_json`, struct field order, no floats/maps) — not over the whole
//! manifest file — so a verifier only needs the one record plus the
//! detached signature to check a single downloaded binary, without
//! parsing or trusting the rest of the manifest.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("binary size mismatch: expected {expected}, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("binary sha256 mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("platform mismatch: expected {expected}, got {actual}")]
    PlatformMismatch { expected: String, actual: String },
    #[error("architecture mismatch: expected {expected}, got {actual}")]
    ArchMismatch { expected: String, actual: String },
    #[error("invalid signature encoding: {0}")]
    InvalidSignatureEncoding(String),
    #[error("signature verification failed")]
    InvalidSignature,
    #[error("json encoding failed: {0}")]
    Encode(#[from] serde_json::Error),
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
}

/// A record plus its detached Ed25519 signature (lowercase hex), as
/// carried in `manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedArtifact {
    #[serde(flatten)]
    pub record: ArtifactRecord,
    pub signature: String,
}

/// The full manifest written alongside a release: one entry per
/// platform/arch, plus the overall release version they belong to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub artifacts: Vec<SignedArtifact>,
}

// --- signing (used by xtask; kept here so the exact byte encoding signed
// and verified can never drift between the two sides) -------------------

pub fn sign_record(record: &ArtifactRecord, signing_key: &ed25519_dalek::SigningKey) -> String {
    use ed25519_dalek::Signer;
    let signature = signing_key.sign(&record.canonical_bytes());
    hex::encode(signature.to_bytes())
}

// --- verification (used by both xtask, for self-checking what it just
// signed, and eventually the agent) --------------------------------------

fn decode_signature(signature_hex: &str) -> Result<ed25519_dalek::Signature> {
    let bytes = hex::decode(signature_hex)
        .map_err(|err| ReleaseError::InvalidSignatureEncoding(err.to_string()))?;
    let bytes: [u8; 64] = bytes
        .try_into()
        .map_err(|_| ReleaseError::InvalidSignatureEncoding("expected 64 bytes".to_string()))?;
    Ok(ed25519_dalek::Signature::from_bytes(&bytes))
}

/// Verify that `signature_hex` is a valid Ed25519 signature by
/// `verifying_key` over `record`'s canonical bytes. Does not touch any
/// binary or platform/arch expectations — see [`verify_binary`] for that.
pub fn verify_record_signature(
    record: &ArtifactRecord,
    signature_hex: &str,
    verifying_key: &ed25519_dalek::VerifyingKey,
) -> Result<()> {
    use ed25519_dalek::Verifier;
    let signature = decode_signature(signature_hex)?;
    verifying_key
        .verify(&record.canonical_bytes(), &signature)
        .map_err(|_| ReleaseError::InvalidSignature)
}

/// Full verification of a downloaded release artifact: signature over the
/// record, platform/arch match what the caller expects, and the binary's
/// actual size + SHA-256 match the signed record. Every check runs (does
/// not short-circuit on the first failure type) is not required by
/// callers; this returns the first failure encountered, checking the
/// signature first since a record that doesn't verify shouldn't be
/// trusted for anything else it claims.
pub fn verify_binary(
    record: &ArtifactRecord,
    signature_hex: &str,
    verifying_key: &ed25519_dalek::VerifyingKey,
    binary_bytes: &[u8],
    expected_platform: &str,
    expected_arch: &str,
) -> Result<()> {
    verify_record_signature(record, signature_hex, verifying_key)?;

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

    let actual_size = binary_bytes.len() as u64;
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

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;
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
        let sig = key.sign(&record.canonical_bytes());
        hex::encode(sig.to_bytes())
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
}
