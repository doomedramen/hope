//! Runtime access to the release-signing public key embedded at build
//! time by `build.rs` (spec §7.7), and a helper to verify a downloaded
//! release artifact against it. Wired up now so it's ready for the actual
//! self-update flow (spec §7.6) in a later milestone; `agent verify-release`
//! exercises it end-to-end today.

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
    let key = trusted_public_key().ok_or_else(|| {
        anyhow::anyhow!(
            "cannot verify release: this is a dev build with no embedded release public key"
        )
    })?;

    let (platform, arch) = current_platform_arch();

    release::verify_binary(record, signature_hex, &key, binary_bytes, platform, arch)
        .map_err(|err| anyhow::anyhow!("release verification failed: {err}"))
}
