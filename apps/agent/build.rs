//! Embeds the trusted release-signing public key (spec §7.7: "the trusted
//! signing public key is embedded in the agent") at build time.
//!
//! Set `HOPE_RELEASE_PUBLIC_KEYS` (comma/whitespace-separated hex-encoded
//! 32-byte Ed25519 public keys) or `HOPE_RELEASE_PUBLIC_KEYS_FILE` (a path to
//! a file containing one key per line). The singular `HOPE_RELEASE_PUBLIC_KEY`
//! and `_FILE` names remain compatible for a one-key build. Never a
//! default/baked-in
//! key: if neither is set, the build compiles a dev-mode agent that has
//! release verification disabled (see `apps/agent/src/release_verify.rs`)
//! and says so loudly at startup, rather than silently trusting nothing
//! or embedding a key that isn't actually meaningful.

use std::env;

fn main() {
    // Required so `#[cfg(hope_dev_no_release_key)]` isn't reported as an
    // unexpected/unknown cfg on modern cargo.
    println!("cargo::rustc-check-cfg=cfg(hope_dev_no_release_key)");
    println!("cargo:rerun-if-env-changed=HOPE_RELEASE_PUBLIC_KEYS");
    println!("cargo:rerun-if-env-changed=HOPE_RELEASE_PUBLIC_KEYS_FILE");
    println!("cargo:rerun-if-env-changed=HOPE_RELEASE_PUBLIC_KEY");
    println!("cargo:rerun-if-env-changed=HOPE_RELEASE_PUBLIC_KEY_FILE");

    let key_source = env::var("HOPE_RELEASE_PUBLIC_KEYS")
        .ok()
        .or_else(|| {
            let path = env::var("HOPE_RELEASE_PUBLIC_KEYS_FILE").ok()?;
            println!("cargo:rerun-if-changed={path}");
            Some(std::fs::read_to_string(&path).unwrap_or_else(|err| {
                panic!("HOPE_RELEASE_PUBLIC_KEYS_FILE={path} could not be read: {err}")
            }))
        })
        .or_else(|| env::var("HOPE_RELEASE_PUBLIC_KEY").ok())
        .or_else(|| {
            let path = env::var("HOPE_RELEASE_PUBLIC_KEY_FILE").ok()?;
            println!("cargo:rerun-if-changed={path}");
            Some(std::fs::read_to_string(&path).unwrap_or_else(|err| {
                panic!("HOPE_RELEASE_PUBLIC_KEY_FILE={path} could not be read: {err}")
            }))
        });

    match key_source {
        Some(source) => {
            let keys = source
                .split(|character: char| character == ',' || character.is_ascii_whitespace())
                .filter(|value| !value.trim().is_empty())
                .map(str::trim)
                .map(|hex| {
                    let bytes = hex::decode(hex).unwrap_or_else(|err| {
                        panic!("HOPE_RELEASE_PUBLIC_KEYS is not valid hex: {err}")
                    });
                    assert_eq!(
                        bytes.len(),
                        32,
                        "release public keys must decode to exactly 32 bytes, got {}",
                        bytes.len()
                    );
                    hex.to_ascii_lowercase()
                })
                .fold(Vec::new(), |mut keys: Vec<String>, key| {
                    if !keys.contains(&key) {
                        keys.push(key);
                    }
                    keys
                });
            assert!(
                !keys.is_empty(),
                "release public key configuration is empty"
            );
            assert!(
                keys.len() <= 8,
                "release public key configuration supports at most 8 keys"
            );
            let keys_hex = keys.join(",");
            println!("cargo:rustc-env=HOPE_RELEASE_PUBLIC_KEYS={keys_hex}");
            println!("cargo:rustc-env=HOPE_RELEASE_PUBLIC_KEY={}", keys[0]);
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
