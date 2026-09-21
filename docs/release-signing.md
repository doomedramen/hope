# Signed agent releases (spec §7.5 / §7.7)

## Overview

Agent binaries are signed with Ed25519. Each artifact has a signature over a
canonical record (version, platform, arch, size, SHA-256, minimum protocol
version), and the complete `manifest.json` has a detached signature. The
manifest also carries signed release metadata: `channel` (`stable` or
`canary`) and optional `release_notes`. A verifier checks the manifest first,
then checks "this exact binary, for this exact platform/arch, at this exact
size and hash" before an update can use it.

Two crates/tools:

- `crates/release` — shared signing/verification logic (`ArtifactRecord`,
  `Manifest`, `ManifestWithMetadata`, manifest verification, `sign_record`,
  and `verify_binary`). Used by `xtask`, the server release repository, and the
  agent verification path.
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
  ARTIFACTS="target/x86_64-unknown-linux-musl/release/agent=linux=amd64"
```

The `just` recipe emits a stable release without notes. Set channel and notes
with `cargo xtask sign` when needed:

```sh
cargo xtask sign \
  --version 0.1.0 \
  --channel canary \
  --release-notes "Test this build before stable rollout." \
  --key data/release-keys/signing-key.hex \
  --out dist/release \
  --artifact target/x86_64-unknown-linux-musl/release/agent=linux=amd64
```

`channel` accepts only `stable` or `canary`. Older manifests without a
`channel` field remain valid and are interpreted as `stable`. `release_notes`
is omitted when absent and is limited to 4096 UTF-8 bytes. Both fields are
covered by the detached signature over the exact manifest bytes.

Each `--artifact` is `PATH=PLATFORM=ARCH`. Produces, under `dist/release/`:

- `manifest.json` — signed channel metadata, optional release notes, and one
  `SignedArtifact` (record + hex signature) per `--artifact`.
- `manifest.json.sig` — a whole-file detached signature over
  `manifest.json`'s exact bytes, as an extra integrity layer over the
  file as shipped (in addition to the per-record signatures inside it).
- `SHA256SUMS` — standard `sha256sum`-format digest list, for manual
  spot-checks independent of the signing tooling (`sha256sum -c
  SHA256SUMS`).
- `agent-<platform>-<arch>` — a copy of each signed binary.
- `public-key.hex` — the matching public verification key for the image.

`xtask sign` first rejects artifacts whose ELF architecture does not match
their declared Linux target, then self-verifies every artifact after signing
(fails loudly at sign time, not at first download, if anything is
inconsistent).

## Embedding the trusted public key in the agent

The agent embeds its trusted public keys at **build time**
(`apps/agent/build.rs`), per spec §7.7 ("the trusted signing public key is
embedded in the agent"):

```sh
HOPE_RELEASE_PUBLIC_KEY_FILE=data/release-keys/public-key.hex cargo build -p agent
# or: HOPE_RELEASE_PUBLIC_KEY=<hex> cargo build -p agent
# rotation build: HOPE_RELEASE_PUBLIC_KEYS_FILE=data/release-keys/trusted-public-keys.hex cargo build -p agent
```

The plural environment variable accepts one hex key per line (or a
comma/whitespace-separated list). Builds deduplicate and bound the list before
embedding it.

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

`.github/workflows/ci.yml` runs on pushes to `main`, builds `linux/amd64`, signs
its manifest, and packages the verified bundle and
public trust key in the matching Hope image. The server serves releases from
that image; installation and updates do not require GitHub access from the
homelab. CI embeds the same release version in the agent's Hello message.
Operators do not manage keys or release files.

- CI requires repository secrets `HOPE_RELEASE_SIGNING_KEY_HEX` and
  `HOPE_RELEASE_PUBLIC_KEY_HEX` and signs with the production release key.
  A `main` push **fails before publishing** if either secret is unavailable.
  The signing command checks that the public
  key matches the private key before producing the bundle.

## Key rotation (spec §7.7)

The server release repository and agent verifier accept a bounded list of
trusted public keys. Configure the server with one key per line in
`HOPE_RELEASE_PUBLIC_KEYS_FILE` (or use the comma/whitespace-separated
`HOPE_RELEASE_PUBLIC_KEYS` environment variable).

1. Generate a new keypair (`cargo xtask keygen`) well before the old key's
   planned retirement.
2. During a transition window, keep both public keys configured and sign
   bundles with the new key. The manifest and each artifact record must verify
   against at least one trusted key.
3. Ship an agent release, signed by the old key, that embeds both the old and
   new public keys as trusted. Every agent must have crossed this dual-trust
   boundary before step 4.
4. Once fleet-wide adoption of the dual-trust build is confirmed, switch
   to signing releases with the new key only.
5. After a further safety window (long enough that no agent is still
   running a pre-dual-trust build), retire the old key from new agent
   builds' embedded trust list.

The complete update and rollback flow, repository layout, and recovery steps
are recorded in [ADR-0017](adr/0017-signed-agent-updates.md).
