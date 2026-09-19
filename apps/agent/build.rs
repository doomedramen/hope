//! Embeds the trusted release-signing public key (spec §7.7: "the trusted
//! signing public key is embedded in the agent") at build time.
//!
//! Set `HOPE_RELEASE_PUBLIC_KEY` (hex-encoded 32-byte Ed25519 public key)
//! or `HOPE_RELEASE_PUBLIC_KEY_FILE` (a path to a file containing that hex
//! string, as produced by `cargo xtask keygen`). Never a default/baked-in
//! key: if neither is set, the build compiles a dev-mode agent that has
//! release verification disabled (see `apps/agent/src/release_verify.rs`)
//! and says so loudly at startup, rather than silently trusting nothing
//! or embedding a key that isn't actually meaningful.

use std::env;

fn main() {
    // Required so `#[cfg(hope_dev_no_release_key)]` isn't reported as an
    // unexpected/unknown cfg on modern cargo.
    println!("cargo::rustc-check-cfg=cfg(hope_dev_no_release_key)");
    println!("cargo:rerun-if-env-changed=HOPE_RELEASE_PUBLIC_KEY");
    println!("cargo:rerun-if-env-changed=HOPE_RELEASE_PUBLIC_KEY_FILE");

    // NOTE: build scripts run with the crate directory (apps/agent) as
    // their working directory, not the workspace root — a relative
    // HOPE_RELEASE_PUBLIC_KEY_FILE is relative to *that*. Use an absolute
    // path (or one relative to apps/agent) to avoid surprises.
    let key_hex = env::var("HOPE_RELEASE_PUBLIC_KEY")
        .ok()
        .or_else(|| {
            let path = env::var("HOPE_RELEASE_PUBLIC_KEY_FILE").ok()?;
            println!("cargo:rerun-if-changed={path}");
            // Fail loudly rather than silently degrading to dev mode: if the
            // operator went to the trouble of setting this var, a missing/
            // unreadable file is a mistake they need to see, not a silent
            // "oh well, no verification then."
            Some(std::fs::read_to_string(&path).unwrap_or_else(|err| {
                panic!("HOPE_RELEASE_PUBLIC_KEY_FILE={path} could not be read: {err}")
            }))
        })
        .map(|s| s.trim().to_string());

    match key_hex {
        Some(hex) => {
            let bytes = hex::decode(&hex).unwrap_or_else(|err| {
                panic!("HOPE_RELEASE_PUBLIC_KEY(_FILE) is not valid hex: {err}")
            });
            assert_eq!(
                bytes.len(),
                32,
                "HOPE_RELEASE_PUBLIC_KEY(_FILE) must decode to exactly 32 bytes, got {}",
                bytes.len()
            );
            println!("cargo:rustc-env=HOPE_RELEASE_PUBLIC_KEY={hex}");
        }
        None => {
            println!("cargo:rustc-cfg=hope_dev_no_release_key");
            println!(
                "cargo:warning=no HOPE_RELEASE_PUBLIC_KEY(_FILE) set — building a DEV agent \
                 with release-signature verification disabled. See docs/release-signing.md."
            );
        }
    }
}
