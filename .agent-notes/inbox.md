
## Agent enrollment accepts any server TLS cert (TOFU)

- Created: 2026-09-19
- Type: concern
- Area: agent
- Context: M0 agent mTLS handshake slice
- Status: captured

Agent enroll request uses `danger_accept_invalid_certs` since agent has no CA cert before enrolling (documented in `apps/agent/src/enroll.rs`). A MITM could impersonate the server and steal the enrollment token. Suggested fix: pin CA SHA-256 fingerprint (kubeadm-style), passed via `--ca-fingerprint` or embedded in token.

## Agent cert renewal not implemented

- Created: 2026-09-19
- Type: todo
- Area: agent
- Context: M0 agent mTLS handshake slice
- Status: captured

Certs have fixed 30d lifetime; agent stops connecting after expiry. TODO noted in pki.rs.

## Agent run has no reconnect/backoff

- Created: 2026-09-19
- Type: todo
- Area: agent
- Context: M0 agent mTLS handshake slice
- Status: captured

`agent run` does not reconnect or back off if the connection drops.

## CA loaded from disk per enroll request

- Created: 2026-09-19
- Type: note
- Area: server
- Context: M0 agent mTLS handshake slice
- Status: captured

Server reads the CA from disk on every enrollment request; no caching.

## Login sessions held in memory

- Created: 2026-09-19
- Type: todo
- Area: auth
- Context: M0 skeleton slice
- Status: captured

Sessions reset on restart (tower-sessions MemoryStore). Should move to Postgres.

## tower-sessions bumped to 0.15

- Created: 2026-09-19
- Type: note
- Area: auth
- Context: M0 skeleton slice
- Status: captured

Version 0.13 pulled in axum-core 0.4, incompatible with axum 0.8, so upgraded to 0.15. Deviation from plan defaults.

## Missing M0 deliverables

- Created: 2026-09-19
- Type: todo
- Area: m0
- Context: M0 skeleton slice
- Status: captured

Threat model, signed development-release pipeline, and real worker job handlers (worker only claims and completes jobs so far).

## No rate limiting on enroll and login

- Created: 2026-09-19
- Type: concern
- Area: auth
- Context: M0 CA pinning / threat model slice
- Status: captured

No rate limiting on `/enroll` or `/api/v1/login`. Spec §12.3 requires rate limiting.

## Revocation doesn't close open agent connections

- Created: 2026-09-19
- Type: concern
- Area: agent
- Context: M0 CA pinning / threat model slice
- Status: captured

Revoking an agent blocks new gateway connections only; already-open sessions stay connected.

## Main API listener is plain HTTP

- Created: 2026-09-19
- Type: note
- Area: server
- Context: M0 CA pinning / threat model slice
- Status: captured

Main API/UI listener has no TLS; documented as requiring a fronting reverse proxy. Agent gateway and enroll listeners do their own TLS.

## CA pinning trust relies on out-of-band fingerprint transfer

- Created: 2026-09-19
- Type: note
- Area: agent
- Context: M0 CA pinning / threat model slice
- Status: captured

Enrollment trust bottoms out on the operator copying the CA fingerprint/enroll string over a trusted channel. Inherent bootstrap limitation, documented in docs/threat-model.md. SSH install flow (M6) should pass it automatically.

## Handshake integration test not re-run after CA pinning

- Created: 2026-09-19
- Type: todo
- Area: testing
- Context: M0 CA pinning / threat model slice
- Status: captured

DB-gated test `enroll_connect_hello_heartbeat_replay_expiry_revocation` could not run: Docker Desktop daemon unresponsive. Re-run against postgres:17 before merging m0-foundation.

## Hand-rolled Postgres session store

- Created: 2026-09-19
- Type: note
- Area: auth
- Context: M0 sessions / rate limiting slice
- Status: captured

`tower-sessions-sqlx-store` 0.15.0 depends on `tower-sessions-core` 0.14, incompatible with `tower-sessions` 0.15, so a custom `PgSessionStore` was written. Not yet exercised against a real Postgres (Docker outage). Revisit if the upstream crate catches up.

## CSRF middleware unproven end-to-end

- Created: 2026-09-19
- Type: todo
- Area: auth
- Context: M0 sessions / rate limiting slice
- Status: captured

CSRF header middleware applied to setup/login, but no cookie-authenticated mutating route exists yet to verify it. Test once the first one lands.

## Rate limiting is per-IP only

- Created: 2026-09-19
- Type: note
- Area: auth
- Context: M0 sessions / rate limiting slice
- Status: captured

No per-account lockout. `X-Forwarded-For` trust is off by default and must be enabled explicitly behind a reverse proxy, otherwise all clients share the proxy's IP bucket.

## Handshake test re-verified; session store still lacks a DB test

- Created: 2026-09-19
- Type: note
- Area: testing
- Context: Docker back after outage
- Status: captured

At ba7965b, full workspace tests passed against postgres:17 incl. `enroll_connect_hello_heartbeat_replay_expiry_revocation` and jobs tests. Hand-rolled `PgSessionStore` has no DB-gated test yet, so still unverified against real Postgres.

## Agent release key rotation not implemented

- Created: 2026-09-19
- Type: todo
- Area: release
- Context: M0 signed release pipeline slice
- Status: captured

Multi-key agent trust for rotation is only planned in docs/release-signing.md. Must be implemented before any real key rotation (spec §7.7) and before first public release.

## CI signing uses ephemeral key without secrets

- Created: 2026-09-19
- Type: note
- Area: release
- Context: M0 signed release pipeline slice
- Status: captured

Without configured signing secrets, CI generates a per-run key (workflow warns). Those artifacts are not trustable releases. Real release key must be set up as a CI secret.

## HOPE_RELEASE_PUBLIC_KEY_FILE path is relative to apps/agent

- Created: 2026-09-19
- Type: note
- Area: release
- Context: M0 signed release pipeline slice
- Status: captured

build.rs resolves the path relative to apps/agent, not repo root. Documented; missing/unreadable file now panics instead of silently falling back to dev mode.

## DB-gated tests must run single-threaded

- Created: 2026-09-19
- Type: concern
- Area: testing
- Context: M0 worker handlers / compose slice
- Status: captured

`jobs::claim()` is table-wide, so parallel DB-gated tests steal each other's jobs. Workaround: `--test-threads=1` in justfile/CI. Better fix later: per-test database/schema, or claim filtered by queue/kind.

## Migration upgrade-path test needed from M1

- Created: 2026-09-19
- Type: todo
- Area: database
- Context: M0 worker handlers / compose slice
- Status: captured

M0 gate "upgrade from previous test schema" not testable yet (no prior schema; see migrations/README.md). When M1 adds migrations, add a test applying through the last tagged schema then new ones on top.

## Compose runs on plain HTTP with insecure cookies

- Created: 2026-09-19
- Type: note
- Area: deploy
- Context: M0 worker handlers / compose slice
- Status: captured

`.env.example` sets `HOPE_COOKIE_SECURE=false` so login works over the compose stack's plain HTTP. Production config needs TLS in front and secure cookies on.

## CI-only M0 gates unverified locally

- Created: 2026-09-19
- Type: todo
- Area: ci
- Context: Local-only development, no remote
- Status: captured

Repo has no remote, so CI never runs. arm64/amd64 musl agent cross-compile (needs cargo-zigbuild, not installed) and the CI signing job are unverified.

## Agent cross-compile verified locally

- Created: 2026-09-19
- Type: note
- Area: ci
- Context: Closing M0 gates locally
- Status: captured

cargo-zigbuild 0.23.4 + zig 0.16.0: agent builds for x86_64/aarch64-unknown-linux-musl (static, 3.9M/3.5M release) and both run `--version` in alpine containers. CI signing job itself still unexercised (no remote).
