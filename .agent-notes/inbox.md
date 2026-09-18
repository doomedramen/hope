
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
