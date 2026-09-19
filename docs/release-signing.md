# Signed agent releases (spec §7.5 / §7.7)

## Overview

Agent binaries are signed with Ed25519. The signature covers a canonical
per-artifact record (version, platform, arch, size, SHA-256, minimum
protocol version) — not just the raw binary — so a verifier checks
"this exact binary, for this exact platform/arch, at this exact size and
hash, was approved for release" in one step, using only the record and its
detached signature (it doesn't need to trust or fully parse the rest of
`manifest.json`).

Two crates/tools:

- `crates/release` — shared signing/verification logic (`ArtifactRecord`,
  `sign_record`, `verify_binary`). Used by `xtask` today; will be used by
  the agent's actual self-update flow in a later milestone (spec §7.6).
- `xtask` — the release tool: `keygen` and `sign`. Not shipped; developer/CI
  tooling only, run via `cargo xtask ...` or the `just release-*` recipes.

## Generating a signing key (development)

```sh
just release-keygen
# or: cargo xtask keygen --out data/release-keys
```

Writes `data/release-keys/signing-key.hex` (private, `0600`, hex-encoded
32-byte Ed25519 seed) and `data/release-keys/public-key.hex` (public).
`data/` is gitignored — **never commit a signing key**. Re-running
`keygen` against an existing `--out` directory refuses to overwrite it.

## Signing a release

```sh
just release-sign VERSION=0.1.0 \
  ARTIFACTS="target/x86_64-unknown-linux-musl/release/agent=linux=amd64 target/aarch64-unknown-linux-musl/release/agent=linux=arm64"
```

Each `--artifact` is `PATH=PLATFORM=ARCH`. Produces, under `dist/release/`:

- `manifest.json` — one `SignedArtifact` (record + hex signature) per
  `--artifact`.
- `manifest.json.sig` — a whole-file detached signature over
  `manifest.json`'s exact bytes, as an extra integrity layer over the
  file as shipped (in addition to the per-record signatures inside it).
- `SHA256SUMS` — standard `sha256sum`-format digest list, for manual
  spot-checks independent of the signing tooling (`sha256sum -c
  SHA256SUMS`).
- `agent-<platform>-<arch>` — a copy of each signed binary.

`xtask sign` self-verifies every artifact immediately after signing
(fails loudly at sign time, not at first download, if anything is
inconsistent).

## Embedding the trusted public key in the agent

The agent embeds its trusted public key at **build time**
(`apps/agent/build.rs`), per spec §7.7 ("the trusted signing public key is
embedded in the agent"):

```sh
HOPE_RELEASE_PUBLIC_KEY_FILE=data/release-keys/public-key.hex cargo build -p agent
# or: HOPE_RELEASE_PUBLIC_KEY=<hex> cargo build -p agent
```

If neither is set, the build still succeeds but emits a `cargo:warning`
and compiles a **dev-mode** agent (`hope_dev_no_release_key` cfg) whose
`release_verify::trusted_public_key()` returns `None` and logs loudly at
runtime if anything tries to verify a release against it. There is no
baked-in default key — a dev build simply cannot verify releases, rather
than silently trusting something meaningless.

Check what a given binary embeds / whether verification works end to end:

```sh
agent verify-release --manifest dist/release/manifest.json --binary dist/release/agent-linux-amd64
```

## CI

`.github/workflows/ci.yml`'s `agent-cross-compile` job builds, signs, and
uploads a release bundle for `linux/amd64` and `linux/arm64` on every
push/PR:

- If the repository secrets `HOPE_RELEASE_SIGNING_KEY_HEX` /
  `HOPE_RELEASE_PUBLIC_KEY_HEX` are configured, CI signs with the real
  release key.
- Otherwise it generates an ephemeral, CI-run-only keypair (via
  `cargo xtask keygen`) so the full pipeline — build, embed, sign,
  self-verify — is exercised on every run, and flags the run with a
  workflow warning so nobody mistakes an ephemeral-signed artifact for a
  trusted release.

## Key rotation plan (spec §7.7)

Not yet implemented — this is the intended design, to build out before any
public release:

1. Generate a new keypair (`cargo xtask keygen`) well before the old key's
   planned retirement.
2. During a transition window, sign each release with **both** the old and
   new key, producing two `SignedArtifact` entries per platform/arch in
   `manifest.json` (or two manifests) — `crates/release`'s data model
   already supports multiple `SignedArtifact`s per binary; `xtask sign`
   would need a `--key` list instead of one path to do this in one pass.
3. Ship an agent release, signed only with the **old** key, that embeds
   **both** the old and new public keys as trusted (agent-side change:
   `trusted_public_key()` becomes `trusted_public_keys() -> Vec<...>`,
   and `verify_release` accepts if any one of them validates). Every
   agent must have upgraded to this dual-trust build before step 4.
4. Once fleet-wide adoption of the dual-trust build is confirmed, switch
   to signing releases with the new key only.
5. After a further safety window (long enough that no agent is still
   running a pre-dual-trust build), retire the old key from new agent
   builds' embedded trust list.

The agent's current build only embeds a single key
(`HOPE_RELEASE_PUBLIC_KEY`); multi-key trust is the concrete gap step 3
above requires before rotation is actually possible without an
availability gap. Track this as a prerequisite for any real-world (non-dev)
rollout.
