# Threat model

Status: reviewed through Milestone 10. M9 adds maintenance reservations,
expected-failure suppression, and overrun delivery. M10 adds operational
backup, restore, upgrade, release, and Compose guidance; it does not add a new
runtime security boundary.

Scope: the control-plane server, PostgreSQL, worker, agent, release
repository, operator browser session, deployment host, and network between
them. Out of scope: physical host compromise, upstream crate or base-image
supply-chain attacks, and security controls provided by an operator's reverse
proxy, secret manager, firewall, or backup platform.

Method: STRIDE per component, with current mitigation and honest gaps. A
documented procedure is not an enforced control; operators must apply it.

## 1. Network scanning and monitoring

Implemented across M2–M4. Scope approval, target-count display, bounded
concurrency and rate limits, safe protocol payloads, and cancellation reduce
the risk of a compromised or misconfigured worker scanning outside the
approved CIDR or overwhelming fragile devices. Scan duration, source,
completeness, observations, and change events remain attributable in
PostgreSQL.

The agent gateway also accepts bounded host metrics and collector data from
enrolled agents. Agent revocation is checked before accepting a new gateway
connection.

## 2. Stored credentials

M6 stores credential ciphertext in PostgreSQL using ChaCha20-Poly1305 and a
32-byte external master key. The normal API returns metadata only. A worker
operation decrypts a selected credential for the fixed SSH install, repair, or
update path; secret-bearing logs and job payloads are redacted or excluded.

The Compose example mounts the key as a read-only container secret at
`/run/secrets/hope_credential_master_key`. The key is not in `.env`, the image,
or PostgreSQL. A database backup without the exact key cannot restore
credential use.

*Gap*: in-place master-key rotation and re-encryption are not implemented.
Keep the old key available until a future controlled migration exists.

*Gap*: an operator or deployment process that can read both the database and
the master key can decrypt credentials. Use separate host permissions and a
secret-management process; the application does not remove that trust.

SSH first-use, changed, and revoked host keys block deployment until the
operator explicitly trusts the durable metadata record. The worker executes a
fixed, quoted command sequence, bounds output and duration, and never accepts
a free-form shell command.

## 3. Enrollment and server identity

The operator creates a short-lived, single-use enrollment token. The command
prints the token, the CA SHA-256 fingerprint, and a combined transfer code.
The agent pins the supplied fingerprint before sending the token or CSR. The
server signs only the submitted public key and derives CA status, usages, and
validity itself. Enrollment is rate-limited per client IP.

The CA key, CA certificate, server certificate, and server key live in the
`server-pki` volume. They are identity material, not disposable container
state. Losing the CA key or replacing the CA changes the fingerprint and
strands existing agents until each agent is re-enrolled.

*Gap*: the fingerprint must cross a trusted operator channel. A compromised
bootstrap channel can defeat pinning before enrollment.

*Gap*: the current client certificate lifetime is 30 days and there is no
renewal flow. Operators must re-enroll before expiry.

*Gap*: revocation blocks the next gateway connection but does not force-close
an already open connection. There is no CRL or OCSP enforcement at the TLS
layer; application checks enforce revocation after the handshake.

## 4. Agent privilege and runtime

Manual installation creates a dedicated non-login service account, a `0700`
state directory, and a root-owned executable. The documented systemd unit
uses `NoNewPrivileges`, `ProtectHome`, and `ProtectSystem`. The agent private
key is `0600` and remains on its host.

*Gap*: collectors may need host-specific read privileges, and root or the same
UID can still read the agent key. No TPM, secure enclave, or hardware-backed
identity is required.

*Gap*: the Compose runtime image does not declare a non-root `USER`. Treat the
container host and its mounted secret/PKI volumes as privileged deployment
surfaces until a later hardening change proves a non-root runtime compatible.

If an agent state directory loses its private key, client certificate, or CA
certificate, that host cannot authenticate or verify the gateway. Re-enroll the
host with a new token; never copy another host's private key.

## 5. Signed agent updates and release repository

M7 verifies the detached manifest signature, each artifact signature, size,
SHA-256, regular-file status, platform, architecture, release version,
protocol floor, and signed channel metadata. The server and worker use the
same filesystem-backed repository. Compose mounts it read-only. The private
Ed25519 signing key remains outside the server and worker.

The configured public-key file or bounded public-key list is trust
configuration. During rotation, both old and new keys can be accepted until
agents cross the dual-trust release boundary. The worker re-verifies the
artifact immediately before SSH transfer, atomically installs it, waits for a
target-version gateway hello, and restores the previous binary when the
check-in deadline fails. A separate repair path exists when rollback cannot
restore service.

If the repository or public trust file is missing, signed release selection and
updates stop; already-running agents keep their current binary. If the private
signing key is missing, existing signed bundles remain verifiable but no new
bundle can be signed under that key. Never replace a missing trust key with an
unreviewed key: that changes the update trust root.

*Gap*: signing-key compromise requires key rotation and fleet recovery. The
update worker cannot revoke a malicious release already trusted by an agent.

*Gap*: release publication is an operator-controlled filesystem step. The
repository is not an internet download service and has no independent
multi-party approval or transparency log.

## 6. Web authentication and transport

`/api/v1/setup` creates the first admin account only when no account exists.
`/api/v1/login` verifies an Argon2 password hash. Setup and login are
rate-limited per client IP. Sessions are stored in PostgreSQL, use `HttpOnly`
and `SameSite=Lax`, expire after 12 hours of inactivity, and are cleaned by a
worker job.

Every unsafe `/api/v1` mutation requires `X-Requested-With: hope` in addition
to the session boundary. M9 maintenance and other authenticated mutation
routes use this middleware. Audit and change-event records attribute operator
and worker actions.

*Gap*: rate limiting is per IP, not per account or distributed identity. There
is no account lockout or distributed attack mitigation.

*Gap*: the main API listener (`/api/v1/*`, `/health/*`, and the web SPA) is
plain HTTP. The Compose example publishes it to localhost by default, but any
non-local deployment must put a TLS reverse proxy in front and keep
`HOPE_COOKIE_SECURE=true`. The 8443 mTLS gateway must use TCP passthrough so
the server sees the client certificate. Preserve the server certificate and CA
fingerprint for enrollment on 8444 as well.

*Gap*: `HOPE_TRUST_PROXY_HEADERS` must remain false unless the proxy strips and
rewrites forwarded headers. A client-controlled `X-Forwarded-For` can otherwise
spoof rate-limit buckets.

## 7. Dependency-aware alerting

M8 stores confirmed dependency edges and bounds graph traversal. When a child
incident notification is suppressed, PostgreSQL retains the child incident,
monitor result, dependency path, target, event, and technical reason. The
operator can inspect the suppression instead of losing evidence.

*Gap*: a wrong confirmed edge or stale operator decision can suppress a useful
downstream notification. Keep confirmation review and graph reconciliation in
the operator workflow; suppression does not replace incident investigation.

## 8. Maintenance planning and overrun delivery

M9 stores events, normalized resources, and expanded occurrences in
PostgreSQL. Creation and edits take a transaction-scoped advisory lock and
compare reservation windows against shared resources, affected/required
relationships, confirmed dependency paths, and the default global disruptive
lock. Optimistic versions prevent stale edits. Recurrence is bounded to a
90-day default horizon and at most 366 configured days; ambiguous or
nonexistent local times are rejected.

Expected-failure resources can suppress only the matching incident notification
for the exact event and occurrence. The incident and recovery remain durable.
An open expected incident past the planned end changes the event to
`overrunning`, retains its reservation, and queues an idempotent
`maintenance.overrun` delivery through configured webhook or ntfy channels.

*Gap*: an operator can intentionally mark a real failure as expected and hide
its downstream notification. The exact suppression record and audit trail make
this reviewable, but they do not prevent misuse.

*Gap*: the M9 backend API exists before a calendar/timeline UI. Operators must
use the API or another client to review conflicts, lifecycle state, and
overruns.

## 9. Backup, restore, and upgrade operations

M10 documents a repeatable logical PostgreSQL dump plus exact restoration of
the credential master key, `server-pki` volume, release repository, public trust
configuration, and deployment environment. Clean restore uses a new Compose
project so the old volumes remain available during validation.

*Gap*: there is no automatic backup, WAL/PITR, backup encryption, immutable
backup retention, or scheduled restore test in the application. Those controls
belong to the operator's backup platform until implemented.

Migrations are forward-only and embedded in the image. The server and worker
can both run them under the SQL migration lock. M10 defines a target of two
supported application versions (`N` and `N-1`), but the repository does not yet
prove every version pair or offer automatic schema downgrade. Treat upgrades as
lockstep server/worker changes with a tested backup and forward-recovery plan.

The Compose server healthcheck proves container liveness. `/health/ready`
checks PostgreSQL reachability and must be used by the reverse proxy or
operator for readiness.

## 10. Telemetry and data egress

Anonymous telemetry, analytics, crash reporting, and phone-home behavior are
off. The repository has no external telemetry endpoint or opt-in telemetry
setting. Agent operational observations flow to the configured Hope control
plane as product functionality, not anonymous telemetry.

Configured webhook and ntfy channels can intentionally send incident,
maintenance-overrun, or other operational payloads to external systems. The
operator controls those destinations and must review their data handling.
Container logs go to the deployment's stdout/logging system; Docker or a host
log shipper can export them independently of Hope.

## Open security gaps

1. Credential master-key rotation and re-encryption are not implemented.
2. Client certificate renewal is not implemented; certificates expire after 30
   days.
3. Revocation does not force-close existing gateway connections and has no
   CRL/OCSP layer.
4. Authentication rate limits are per IP only.
5. The main API has no native TLS and requires a correctly configured proxy for
   non-local use.
6. The Compose runtime image has no explicit non-root user.
7. Backups have no application-provided encryption, PITR, automation, or
   restore-test scheduler.
8. Migrations have no automatic downgrade, and the two-version target is not
   yet a tested compatibility guarantee.
9. Release signing-key compromise and publication approval remain operational
   responsibilities.
10. Maintenance expected-failure and dependency confirmations can be misused;
    audit records make misuse visible but do not prevent it.

Resolved or materially improved through M10: signed artifact verification and
agent binary rollback (M7), dependency-path alert suppression records (M8),
maintenance conflict/lifecycle/overrun handling (M9), PostgreSQL-backed
sessions and periodic cleanup, per-IP setup/login/enrollment limits, and the
CSRF custom-header mechanism. None of these resolutions removes the gaps
listed above.
