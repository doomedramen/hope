//! Runtime access to release-signing keys embedded at build time by `build.rs`
//! (spec §7.7), plus release-bundle verification helpers. The old
//! record-level API remains available to the current `VerifyRelease` command;
//! callers with the detached `manifest.json.sig` bytes should use
//! [`verify_release_manifest`] for complete bundle verification.

use ed25519_dalek::VerifyingKey;

/// The embedded trusted public key, or `None` in a dev build that had no
/// `HOPE_RELEASE_PUBLIC_KEY(_FILE)` set at compile time.
pub fn trusted_public_key() -> Option<VerifyingKey> {
    #[cfg(hope_dev_no_release_key)]
    {
        tracing::warn!(
            "DEV MODE: no release public key was embedded at build time; \
             release-signature verification is DISABLED. Do not use this \
             build to accept signed release updates."
        );
        None
    }

    #[cfg(not(hope_dev_no_release_key))]
    {
        let hex = env!("HOPE_RELEASE_PUBLIC_KEY");
        let bytes: [u8; 32] = hex::decode(hex)
            .expect("HOPE_RELEASE_PUBLIC_KEY was validated as hex at build time")
            .try_into()
            .expect("HOPE_RELEASE_PUBLIC_KEY was validated as 32 bytes at build time");
        Some(VerifyingKey::from_bytes(&bytes).expect("embedded key must be a valid Ed25519 key"))
    }
}

/// Return this build's bounded trusted-key rotation set.
///
/// Current builds embed one key through `HOPE_RELEASE_PUBLIC_KEY`. Returning a
/// list keeps verification APIs rotation-ready: a transition build can pass
/// old and new keys to the explicit `*_with_trusted_keys` helpers while this
/// compatibility function preserves current embedded-key behavior.
pub fn trusted_public_keys() -> Option<Vec<VerifyingKey>> {
    trusted_public_key().map(|key| vec![key])
}

/// This build's platform/arch, in the same vocabulary `xtask sign` and
/// the CI matrix use (`linux`/`amd64`|`arm64`), for matching against a
/// manifest's artifact records.
pub fn current_platform_arch() -> (&'static str, &'static str) {
    let platform = if cfg!(target_os = "linux") {
        "linux"
    } else {
        std::env::consts::OS
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    (platform, arch)
}

/// Verify a downloaded `(manifest_record, signature_hex, binary_bytes)`
/// release artifact against the embedded trusted key for this build's
/// platform/arch. Returns an error (rather than panicking) if this is a
/// dev build with no embedded key — callers must not silently proceed as
/// if verification passed.
pub fn verify_release(
    record: &release::ArtifactRecord,
    signature_hex: &str,
    binary_bytes: &[u8],
) -> anyhow::Result<()> {
    let keys = trusted_public_keys().ok_or_else(|| {
        anyhow::anyhow!(
            "cannot verify release: this is a dev build with no embedded release public key"
        )
    })?;

    verify_release_with_trusted_keys(record, signature_hex, binary_bytes, &keys)
}

/// Verify one artifact against an explicit bounded trusted-key rotation set.
pub fn verify_release_with_trusted_keys(
    record: &release::ArtifactRecord,
    signature_hex: &str,
    binary_bytes: &[u8],
    trusted_keys: &[VerifyingKey],
) -> anyhow::Result<()> {
    let (platform, arch) = current_platform_arch();

    release::verify_binary_with_keys(
        record,
        signature_hex,
        trusted_keys,
        binary_bytes,
        platform,
        arch,
        protocol::PROTOCOL_VERSION,
    )
    .map_err(|err| anyhow::anyhow!("release verification failed: {err}"))
}

/// Verify detached manifest bytes and the current platform/architecture
/// artifact against this build's embedded trusted key set.
#[allow(dead_code)]
pub fn verify_release_manifest(
    manifest_bytes: &[u8],
    manifest_signature_hex: &str,
    binary_bytes: &[u8],
) -> anyhow::Result<release::SignedArtifact> {
    let keys = trusted_public_keys().ok_or_else(|| {
        anyhow::anyhow!(
            "cannot verify release: this is a dev build with no embedded release public key"
        )
    })?;

    verify_release_manifest_with_trusted_keys(
        manifest_bytes,
        manifest_signature_hex,
        binary_bytes,
        &keys,
    )
}

/// Verify a complete release bundle against an explicit trusted-key rotation
/// set. This verifies the detached manifest signature before selecting an
/// artifact, then enforces artifact signature, platform/arch, protocol, size,
/// and SHA-256 checks.
#[allow(dead_code)]
pub fn verify_release_manifest_with_trusted_keys(
    manifest_bytes: &[u8],
    manifest_signature_hex: &str,
    binary_bytes: &[u8],
    trusted_keys: &[VerifyingKey],
) -> anyhow::Result<release::SignedArtifact> {
    let (platform, arch) = current_platform_arch();

    release::verify_manifest_and_binary(
        manifest_bytes,
        manifest_signature_hex,
        trusted_keys,
        binary_bytes,
        platform,
        arch,
        protocol::PROTOCOL_VERSION,
    )
    .map_err(|err| anyhow::anyhow!("release verification failed: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    #[test]
    fn explicit_rotation_keys_accept_new_release_key() {
        let old_key = signing_key(1);
        let new_key = signing_key(2);
        let binary = b"agent binary";
        let (platform, arch) = current_platform_arch();
        let record = release::ArtifactRecord {
            version: "0.1.0".to_string(),
            platform: platform.to_string(),
            arch: arch.to_string(),
            size: binary.len() as u64,
            sha256: release::ArtifactRecord::sha256_of(binary),
            min_protocol_version: protocol::PROTOCOL_VERSION,
        };
        let signature = hex::encode(new_key.sign(&record.canonical_bytes()).to_bytes());
        let trusted_keys = vec![old_key.verifying_key(), new_key.verifying_key()];

        verify_release_with_trusted_keys(&record, &signature, binary, &trusted_keys)
            .expect("rotation set should accept signature from new key");
    }

    #[test]
    fn explicit_keys_enforce_minimum_protocol() {
        let key = signing_key(3);
        let binary = b"agent binary";
        let (platform, arch) = current_platform_arch();
        let record = release::ArtifactRecord {
            version: "0.1.0".to_string(),
            platform: platform.to_string(),
            arch: arch.to_string(),
            size: binary.len() as u64,
            sha256: release::ArtifactRecord::sha256_of(binary),
            min_protocol_version: protocol::PROTOCOL_VERSION + 1,
        };
        let signature = release::sign_record(&record, &key);

        let err =
            verify_release_with_trusted_keys(&record, &signature, binary, &[key.verifying_key()])
                .expect_err("newer protocol requirement must fail");
        assert!(err.to_string().contains("requires protocol version"));
    }

    #[test]
    fn manifest_helper_verifies_detached_signature_and_artifact() {
        let key = signing_key(4);
        let binary = b"agent binary";
        let (platform, arch) = current_platform_arch();
        let record = release::ArtifactRecord {
            version: "0.1.0".to_string(),
            platform: platform.to_string(),
            arch: arch.to_string(),
            size: binary.len() as u64,
            sha256: release::ArtifactRecord::sha256_of(binary),
            min_protocol_version: protocol::PROTOCOL_VERSION,
        };
        let manifest = release::Manifest {
            version: record.version.clone(),
            artifacts: vec![release::SignedArtifact {
                record: record.clone(),
                signature: release::sign_record(&record, &key),
            }],
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("manifest should encode");
        let manifest_signature = release::sign_manifest(&manifest_bytes, &key);

        let verified = verify_release_manifest_with_trusted_keys(
            &manifest_bytes,
            &manifest_signature,
            binary,
            &[key.verifying_key()],
        )
        .expect("complete release bundle should verify");
        assert_eq!(verified.record, record);
    }
}
