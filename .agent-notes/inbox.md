
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
