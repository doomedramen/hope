set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

fmt:
    cargo fmt --all
    pnpm -C apps/web format

lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    pnpm -C apps/web lint

test:
    cargo test --workspace -- --test-threads=1
    pnpm -C apps/web test -- --run

dev:
    cargo run -p server -- serve

web:
    pnpm -C apps/web dev

migrate:
    cargo run -p server -- migrate

# Generate a dev release-signing keypair under data/release-keys (never
# commit it -- see docs/release-signing.md).
release-keygen:
    cargo run -p xtask -- keygen --out data/release-keys

# Sign one or more built agent binaries. VERSION=0.1.0
# ARTIFACTS="path/to/agent=linux=amd64 path/to/agent=linux=arm64"
release-sign VERSION ARTIFACTS:
    #!/usr/bin/env bash
    set -euo pipefail
    args=()
    for artifact in {{ ARTIFACTS }}; do
        args+=("--artifact" "$artifact")
    done
    cargo run -p xtask -- sign \
        --version "{{ VERSION }}" \
        --key data/release-keys/signing-key.hex \
        --out dist/release \
        "${args[@]}"

# Cross-compile both agent targets (embedding the dev public key) and
# sign them in one step.
release-build-and-sign VERSION:
    HOPE_RELEASE_PUBLIC_KEY_FILE=data/release-keys/public-key.hex \
        cargo zigbuild --release -p agent --target x86_64-unknown-linux-musl
    HOPE_RELEASE_PUBLIC_KEY_FILE=data/release-keys/public-key.hex \
        cargo zigbuild --release -p agent --target aarch64-unknown-linux-musl
    just release-sign {{ VERSION }} \
        "target/x86_64-unknown-linux-musl/release/agent=linux=amd64 target/aarch64-unknown-linux-musl/release/agent=linux=arm64"
