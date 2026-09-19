# Threat model (Milestones 0–6)

Status: reviewed through Milestone 6. Revisit at each later milestone as
agent updates, dependency-aware alerting, and maintenance execution land.

Scope: the control-plane server, the agent, the network between them, and
the operator's browser session. Out of scope for this pass: supply-chain
attacks on upstream crates/base images, and physical access to the host
running the server.

Method: STRIDE per component, with current mitigation and honest gaps.

## 1. Network scanning (discovery/monitoring subsystem)

Implemented across M2–M4. The scope and rate-limit controls in spec
§6.1/§6.3 mitigate a compromised or misconfigured scanner causing
collateral damage (scanning networks outside the approved CIDR, overwhelming
fragile devices, or triggering IDS/IPS on someone else's network).

- **Tampering / DoS**: unbounded or unapproved-scope scanning.
  *Mitigation*: explicit scope approval, target-count display before first
  scan, bounded concurrency/rate limits (§6.1, §6.3), and cancellation.
- **Repudiation**: scan activity not attributable.
  *Mitigation*: scan duration/source/completeness retained (§6.3).

## 2. Stored credentials

Implemented in M6 through the encrypted `credentials` resource and external
master-key source. Threats and current mitigations:

- **Information disclosure**: credentials readable by anyone with DB
  access, or exposed via API responses.
  *Mitigation*: ChaCha20-Poly1305 ciphertext is stored in PostgreSQL with a
  32-byte key supplied only through `HOPE_CREDENTIAL_MASTER_KEY` or a mounted
  file. The normal API returns metadata only; decryption is limited to a
  worker operation and secret-bearing debug/log paths are redacted.
- **Elevation of privilege**: a credential meant for one narrow use
  (e.g. a read-only SNMP community string) reused more broadly than
  intended.
  *Gap*: in-place key rotation is documented but not yet implemented; keep
  the old key available until a re-encryption migration exists.

- **SSH MITM / host replacement**: an installer could accept a forged host
  key on first use or after replacement.
  *Mitigation*: first-seen, changed, and revoked keys block the SSH job. The
  operator must explicitly trust the metadata record before a retry can
  authenticate.

- **Remote command injection / output exfiltration**: a target or credential
  could cause arbitrary shell execution or unbounded job logs.
  *Mitigation*: the M6 worker sends a fixed command sequence, quotes all
  operator/config values, caps captured output at 64 KiB, bounds job time, and
  never accepts a free-form command field. Enrollment codes are sent on SSH
  stdin rather than as process arguments.

## 3. Enrollment (agent bootstrap, ADR-0007)

This is the most security-relevant thing the agent bootstrap path implements.

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
  *Mitigation*: the `/enroll` endpoint is rate limited per client IP (20
  requests/minute, `apps/server/src/ratelimit.rs`), bounding how many
  guesses an attacker gets while a token's TTL window is open. Given token
  entropy (32 random bytes), this is comfortably defense-in-depth rather
  than the primary control.
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
  *Mitigation*: per-IP rate limiting (above). *Gap*: the limiter's client-IP
  determination trusts the raw TCP peer address by default; behind a
  reverse proxy, `trust_proxy_headers` must be explicitly enabled (and the
  proxy must strip any client-supplied `X-Forwarded-For`) or every request
  appears to come from the proxy's IP and shares one bucket. Not yet
  load-tested at scale.

## 4. Agent privilege and runtime

- **Elevation of privilege**: the agent process, once installed, has
  whatever OS-level privilege the operator grants it. M6's installer creates
  a dedicated non-login service account and a `0700` state directory, but
  host-specific collectors may still need additional read privileges.
  *Mitigation*: the installer never grants sudo or arbitrary command
  execution; the service unit uses `NoNewPrivileges`, `ProtectHome`, and
  `ProtectSystem`.
- **Tampering (of agent identity)**: agent private key at
  `/var/lib/hope/agent-key.pem` (default state dir) readable only by
  the key's owner (`0600`), but anyone with root or the same UID can read
  it. No hardware-backed key storage (TPM, secure enclave) is required in
  this milestone.
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

Centralized signed update distribution is not implemented yet; it is the
Milestone 7 scope. The current release verification primitives require
operators to provide verified artifact paths to the M6 installer.

- **Tampering**: a compromised build step or artifact host could serve a
  malicious agent binary as if it were official.
  *Mitigation*: the agent and xtask contain Ed25519 release verification
  primitives, but M7 still needs the repository/cache, rollout policy, and
  enforcement in the update/repair workflow.

## 6. Web authentication

- **Spoofing / credential stuffing**: `/api/v1/setup` creates the single
  admin account (only when none exists) with an argon2 password hash;
  `/api/v1/login` verifies against it.
  *Mitigation*: argon2 (memory-hard, resistant to GPU cracking) via the
  `argon2` crate's defaults, plus per-IP rate limiting (10 requests/minute
  on each of `/api/v1/setup` and `/api/v1/login`, same mechanism as
  `/enroll` — see `apps/server/src/ratelimit.rs`).
  *Gap*: rate limiting is per-IP, not per-account, so an attacker
  distributed across many IPs (or behind carrier-grade NAT sharing one IP
  with legitimate users) isn't meaningfully slowed. No account lockout.
- **Session handling**: `tower-sessions` with a hand-rolled Postgres-backed
  `SessionStore` (`apps/server/src/session_store.rs` — `tower-sessions-sqlx-store`
  0.15.0 depends on `tower-sessions-core` 0.14, incompatible with our
  `tower-sessions` 0.15/`tower-sessions-core` 0.15, so it doesn't actually
  satisfy `SessionManagerLayer`; rolled our own against a `sessions`
  table instead). An hourly background task deletes expired rows
  (`Role::Serve`'s `session_cleanup_task` in `main.rs`). The cookie is
  `HttpOnly`, `SameSite=Lax`, and `Secure` (configurable via
  `cookie_secure`, defaulting to `true`; set to `false` only for
  plain-HTTP local dev, where a browser would otherwise silently drop a
  `Secure` cookie).
  *Gap*: sessions now survive a restart and work across replicas sharing
  the DB (previously flagged as a gap; resolved this slice). No
  session-fixation-specific handling beyond what `tower-sessions` does by
  default (new session ID issued on login isn't explicitly verified).
- **Transport**: the main HTTP API (`/api/v1/*`, `/health/*`) currently
  serves plain HTTP, not TLS — unlike the gateway/enroll listeners.
  *Gap*: session cookies and login credentials travel in cleartext unless
  TLS is terminated in front of the server (e.g. a reverse proxy), and
  `cookie_secure` must then be left at its default `true` so the browser
  won't send the cookie over that same plain-HTTP hop by mistake. This is
  a real gap for any non-localhost deployment; documenting the expectation
  that operators front the API with TLS (or that a future milestone adds
  it directly) is necessary before this ships beyond a dev environment.
- **CSRF**: mitigated via two layers (`apps/server/src/csrf.rs`): the
  session cookie is `SameSite=Lax` (stops it riding along on most
  cross-site requests), and a middleware requires a custom
  `X-Requested-With: hope` header on every unsafe-method (`POST`/`PUT`/
  `PATCH`/`DELETE`) request under `/api/v1/*` — a plain cross-site
  form/image/link CSRF attack cannot attach custom headers, and a
  cross-origin `fetch` that tried to would need a CORS preflight the
  server doesn't grant.
  *Gap*: no cookie-authenticated *mutating* route exists yet to actually
  exercise this against (`/api/v1/setup` and `/api/v1/login` establish a
  session rather than using one, so classic CSRF's premise doesn't fully
  apply to them). The mechanism is verified to compile and apply to those
  two routes, but hasn't been proven end-to-end against a real
  authenticated mutation — do that as soon as the first such route lands.

## Summary of open gaps (tracked for follow-up milestones)

1. Credential master-key rotation is not implemented yet.
2. Rate limiting is per-IP only (no per-account lockout, no
   distributed-attack or shared-NAT mitigation).
3. Revocation doesn't force-close already-open gateway connections.
4. No signed-release / checksum pipeline for agent binaries.
5. Main API listener has no TLS of its own (assumes a fronting proxy);
   `cookie_secure` must stay `true` in that setup.
6. CSRF middleware exists but has no real mutating route to prove itself
   against yet — verify end-to-end once one lands.
7. No least-privilege guidance for the agent's OS-level install.
8. Agent client cert has a fixed 30-day lifetime with no renewal flow yet
   (see `apps/server/src/pki.rs`).

Resolved this slice (previously listed here): sessions were in-memory
only — now Postgres-backed with expiry cleanup; `/enroll` and
`/api/v1/login`+`/setup` had no rate limiting — now per-IP limited;
CSRF had no defenses at all — now has SameSite cookie + custom-header
middleware (see gap 6 above for what's still unverified about it).
