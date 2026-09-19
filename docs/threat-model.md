# Threat model (Milestone 0)

Status: initial, covers what exists at the end of Milestone 0 (repository
and engineering foundation — spec §17). Revisit at each later milestone as
discovery, monitoring, and maintenance subsystems land.

Scope: the control-plane server, the agent, the network between them, and
the operator's browser session. Out of scope for this pass: supply-chain
attacks on upstream crates/base images, and physical access to the host
running the server.

Method: STRIDE per component, with current mitigation and honest gaps.

## 1. Network scanning (discovery/monitoring subsystem)

Not implemented in M0 (spec §17 explicitly excludes it — "Not included:
real scanning, monitors, or inventory"). Noted here because the scope and
rate-limit controls in spec §6.1/§6.3 are the primary planned mitigation
for a compromised or misconfigured scanner causing collateral damage
(scanning networks outside the approved CIDR, overwhelming fragile
devices, or triggering IDS/IPS on someone else's network). Threats to
track when this lands:

- **Tampering / DoS**: unbounded or unapproved-scope scanning.
  *Planned mitigation*: explicit scope approval, target-count display
  before first scan, bounded concurrency/rate limits (§6.1, §6.3).
- **Repudiation**: scan activity not attributable.
  *Planned mitigation*: scan duration/source/completeness retained (§6.3).

## 2. Stored credentials

Not implemented in M0 — no `credentials` resource exists yet (spec §14.1
lists it for a later milestone). Threats to track:

- **Information disclosure**: credentials readable by anyone with DB
  access, or exposed via API responses.
  *Planned mitigation*: spec §13.3 requires device pages to show
  "credential associations without secret values" — secrets must never
  round-trip through the API once stored.
- **Elevation of privilege**: a credential meant for one narrow use
  (e.g. a read-only SNMP community string) reused more broadly than
  intended.
  *Gap*: no secrets-at-rest design exists yet; this needs its own ADR
  before the credentials resource is built (likely encryption at rest with
  a key outside the database, e.g. via `secrets` crate placeholder in
  spec §5.2's suggested layout).

## 3. Enrollment (agent bootstrap, ADR-0007)

This is the most security-relevant thing M0 actually implements.

**Flow**: operator runs `server enroll-token create`, which prints a
single-use token, the CA's SHA-256 fingerprint, and a combined
`token.fingerprint` code. The operator transfers that out-of-band (SSH,
password manager, etc.) to the target host. `agent enroll` uses the
fingerprint to pin the enroll TLS connection, then sends the token + a
locally-generated CSR; the server verifies and consumes the token
atomically, signs the CSR, and returns a client cert + the CA cert.

- **Spoofing (of the server)**: an attacker positioned to intercept the
  enroll HTTPS connection (network MITM, DNS hijack, rogue AP) could
  serve their own TLS cert and harvest the token.
  *Mitigation*: the agent pins the CA SHA-256 fingerprint (supplied by the
  operator out-of-band) via a custom `rustls::client::danger::ServerCertVerifier`
  and rejects the handshake — before the token is ever sent — if the
  presented chain doesn't include a certificate matching that fingerprint.
  This replaced an earlier TOFU (`danger_accept_invalid_certs`) design in
  this same milestone.
  *Gap*: the fingerprint itself must be transferred over a channel the
  operator trusts (SSH session, secrets manager). If that channel is
  compromised, pinning doesn't help — this is inherent to any
  bootstrap-of-trust problem and is judged acceptable for a homelab-scale
  v1.
- **Spoofing (of the agent) / replay**: an attacker who intercepts a
  token could enroll their own device as if it were the real one.
  *Mitigation*: tokens are single-use, consumed atomically
  (`update ... where used_at is null and expires_at > now()`, so a
  concurrent replay cannot also succeed), and time-limited (operator-set
  TTL, default 15 minutes). Enrollment doesn't require client auth (the
  agent has no cert yet), so this token is the only enrollment-time
  secret — treat it like a password.
  *Gap*: no rate limiting on enroll attempts; an attacker who can guess or
  brute-force a token before it expires isn't currently slowed down beyond
  network latency. Low risk given token entropy (32 random bytes) but
  worth a rate limiter later.
- **Tampering**: a CSR requesting attributes beyond "this is a client
  cert for this key" (e.g. requesting CA:true).
  *Mitigation*: the server ignores/overwrites `is_ca`, key usage, and
  validity fields on the incoming CSR params before signing — it only
  trusts the public key and re-derives everything else itself.
- **Information disclosure**: tokens or private keys in logs.
  *Mitigation*: tokens are printed via `println!`, deliberately bypassing
  `tracing` (which emits structured JSON that may be shipped to a log
  aggregator); private keys are never logged and are written to disk with
  `0600` permissions.
- **Denial of service**: the enroll listener has no auth prior to a valid
  token, so it's reachable by anyone who can route to it.
  *Gap*: no request-rate limiting on the enroll endpoint yet (any
  DATABASE_URL-connected server absorbs a flood of invalid-token POSTs
  fine functionally, but this hasn't been load-tested).

## 4. Agent privilege and runtime

- **Elevation of privilege**: the agent process, once installed, has
  whatever OS-level privilege the operator grants it. M0 has no
  install/systemd-unit story yet — `agent enroll`/`agent run` are run
  manually.
  *Gap*: no documented least-privilege recommendation yet (e.g. running as
  a dedicated non-root user with read access only to what discovery/
  monitoring actually need). This belongs in the M2+ agent-capabilities
  work, not M0.
- **Tampering (of agent identity)**: agent private key at
  `/var/lib/hope/agent-key.pem` (default state dir) readable only by
  the key's owner (`0600`), but anyone with root or the same UID can read
  it. No hardware-backed key storage (TPM, secure enclave) in M0.
- **Spoofing (revoked agent reconnecting)**: `server revoke-agent`
  marks an agent's cert revoked in the `agents` table.
  *Mitigation*: the gateway checks `revoked_at` on every new connection
  (after mTLS handshake, before accepting the WebSocket) and closes the
  connection if set.
  *Gap*: revocation doesn't force-close an *already open* connection —
  only blocks the next connection attempt. Also, there's no CRL/OCSP at
  the TLS layer itself, so a revoked cert still completes a valid mTLS
  handshake; revocation is enforced at the application layer only.

## 5. Update supply chain

Not implemented in M0 (signed-release pipeline is a stated M0 deliverable
in spec §17 but the CI cross-compile job doesn't yet sign or publish
artifacts — see `.github/workflows/ci.yml`, which builds but doesn't
publish agent binaries).

- **Tampering**: a compromised build step or artifact host could serve a
  malicious agent binary as if it were official.
  *Gap*: no signing key, no checksum publication, no verification step on
  the agent side. This is explicitly flagged as missing and should block
  any real-world (non-dev) rollout of agent binaries until addressed.

## 6. Web authentication

- **Spoofing / credential stuffing**: `/api/v1/setup` creates the single
  admin account (only when none exists) with an argon2 password hash;
  `/api/v1/login` verifies against it.
  *Mitigation*: argon2 (memory-hard, resistant to GPU cracking) via the
  `argon2` crate's defaults.
  *Gap*: no rate limiting or lockout on `/api/v1/login` — brute force is
  only slowed by argon2's cost, not blocked.
- **Session handling**: `tower-sessions` with `MemoryStore`.
  *Gap*: sessions are in-process memory only — they don't survive a
  server restart (forces re-login, mildly annoying but not a security
  issue) and don't work across multiple server replicas (not a concern
  for the M0 single-process deployment shape, but flag before any
  horizontal scaling). A Postgres-backed session store is the natural
  fix and isn't hard, just not done yet.
- **Transport**: the main HTTP API (`/api/v1/*`, `/health/*`) currently
  serves plain HTTP, not TLS — unlike the gateway/enroll listeners.
  *Gap*: session cookies and login credentials travel in cleartext unless
  TLS is terminated in front of the server (e.g. a reverse proxy). This is
  a real gap for any non-localhost deployment; documenting the expectation
  that operators front the API with TLS (or that a future milestone adds
  it directly) is necessary before this ships beyond a dev environment.
- **CSRF**: no CSRF token scheme yet; relies on the API being JSON-only
  (not form-encoded) as a partial mitigant, which is weak on its own.
  *Gap*: worth a proper CSRF story (e.g. `SameSite=Strict` cookies, which
  `tower-sessions` supports but isn't explicitly configured yet) before
  the UI does more than the current read-only shell.

## Summary of open gaps (tracked for follow-up milestones)

1. No secrets-at-rest design for the future `credentials` resource.
2. No rate limiting on `/enroll` or `/api/v1/login`.
3. Revocation doesn't force-close already-open gateway connections.
4. No signed-release / checksum pipeline for agent binaries.
5. Sessions are in-memory only (no persistence, no multi-replica support).
6. Main API listener has no TLS of its own (assumes a fronting proxy).
7. No CSRF-specific defenses beyond JSON-only content type.
8. No least-privilege guidance for the agent's OS-level install.
9. Agent client cert has a fixed 30-day lifetime with no renewal flow yet
   (see `apps/server/src/pki.rs`).
