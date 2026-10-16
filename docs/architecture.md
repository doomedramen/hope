# Architecture

H.O.P.E is a single Rust server (Axum, Tokio, SQLx on SQLite) that serves a statically exported Next.js console, discovers hosts on up to a handful of IPv4 ranges, and receives reports from a small Rust agent on Linux and macOS machines. Everything below is derived from the code at the paths given; see [operations.md](operations.md) for running it and [validation.md](validation.md) for what has been verified.

```
 browser (static console)          agent hosts (Linux/macOS)
        |  HTTPS via reverse proxy          |  HTTPS POST every ~30 s
        v                                   v
 +----------------------------- hope-server (one process) -----------------------------+
 | auth/limits/headers  api  scanner+arp  enrichment  agents(enroll,report)  metrics   |
 | ssh (install jobs)   releases (manifest, installer)  lifecycle  backup  migrations   |
 +--------------------------------- SQLite (WAL) --------------------------------------+
```

## Components

### Workspace (`Cargo.toml`)

| Crate | Role |
| --- | --- |
| `crates/hope-protocol` | Versioned DTOs shared by server and agent (`SCHEMA_VERSION`), with ts-rs derives. `hope-server export-types` prints the TypeScript contract; `scripts/generate-types.sh` (`make types`) writes `web/lib/protocol.ts` through a temporary file so a failed run never replaces it. CI fails on drift |
| `crates/hope-server` | The server library and binary (modules below) |
| `crates/hope-agent` | The reporting agent binary `hope-agent` |
| `web/` | Next.js App Router, shadcn/ui, Tailwind, Recharts. `npm run build` produces a static export in `web/out`; the Rust server serves it with `tower_http::services::ServeDir`. **Production needs no Node process** |

### Server modules (`crates/hope-server/src`)

| Module | Responsibility |
| --- | --- |
| `main.rs`, `lib.rs` | Configuration from environment, router assembly and middleware order (`headers` -> `limits::client_key` -> `auth::check_origin` -> 256 KiB body limit), first-run token, monitoring and retention timers. Subcommands `export-types` and `backup` |
| `state.rs`, `migrations.rs`, `schema.sql`, `migrations/` | Shared state (SQLite pool, one global write lock, shutdown token, event broadcast), hashing helpers, ordered transactional migrations selected by `PRAGMA user_version` (adopts pre-migration databases, refuses newer ones) |
| `auth.rs` | Setup, login, logout, sessions, same-origin check |
| `limits.rs` | Per-client failure budgets, trusted-proxy `X-Forwarded-For` handling, Argon2 concurrency cap |
| `headers.rs` | Security headers, `no-store` on `/api/`, fixed error bodies |
| `api.rs` | Networks, hosts bootstrap, manual merge, map (atomic edits with revisions), SSE `/events` |
| `scanner.rs`, `arp.rs` | Range validation (`/20` or smaller), fair concurrent scheduler, ICMP/TCP/ARP probes, capability detection |
| `enrichment.rs` | Bounded deduplicated queue of eight workers for service ports and reverse DNS |
| `agents.rs` | Enrollment tokens, install command, enrollment, report ingestion, revocation, identity matching |
| `metrics.rs` | History queries and retention (raw to five-minute rollups, expiry) |
| `validation.rs` | Bounds on inventory, metrics and collector status (finite numbers, string and list limits) |
| `releases.rs` | Signed manifest parsing and availability, installer template with embedded public key, artifact serving |
| `ssh.rs` | SSH installation jobs: host-key binding, strict progress grammar, vetted message catalog, success evidence from the database |
| `lifecycle.rs`, `backup.rs` | Graceful shutdown (8 s deadline) and `VACUUM INTO` backups |

### Agent (`crates/hope-agent/src`)

`main.rs` dispatches `run`, `enroll`, `configure`, `once`, `inspect`, `status`, `check-config`, `restore-config`, `benchmark`, `--version`. `collect.rs` samples CPU, memory, network, filesystem, disk I/O, GPU and inventory with per-collector costs. `reporting.rs` is a single actor that samples every 30 s (28 to 32 s jitter) while one HTTP request may be in flight. `delivery.rs` owns the on-disk spool (at most 120 reports, 4 MiB, one hour; corrupt or future-dated files are rejected) and the retry policy (Retry-After, bounded exponential backoff, revocation stops the agent). `config.rs` stores identity and credential atomically with `0700`/`0600` permissions and an exclusive process lock. `status.rs` proves a report was accepted from a specific service process and activation.

## Data flows

### Discovery to hosts

1. An administrator adds a range (`POST /api/v1/networks`; `PUT`/`DELETE /api/v1/networks/{id}` pause or remove it) and starts a scan, or the 300-second monitoring timer in `main.rs` starts one for every enabled range.
2. `scanner::start` creates a scan row and queues a batch of addresses in the shared `Scheduler`. The scheduler interleaves batches (one address per range in turn), runs at most 64 host probes at once, and a `ProbeLimiter` caps probe starts at 200 per second and sockets at 64 across all ranges.
3. Per address `NativeProbe` runs ICMP, TCP 22/80/443 and (on-link, Linux, native mode) ARP concurrently under a six-second budget. A host is alive when ICMP, ARP or any TCP connect or refusal answered; unanswered probes record nothing and never mark a host offline.
4. `persist_observation` creates or updates the host (rechecking cancellation under the write lock) and enqueues it in `enrichment`, whose workers add TCP service ports and a reverse-DNS name, visible as the host's enrichment state.
5. Changes broadcast an event; browsers receive it over SSE (`/api/v1/events`), reload on a `resync` event after overrun, and also poll every 15 s.

### Agent enrollment and reporting

1. `POST /api/v1/enrollment-tokens` stores a SHA-256 of a random token valid 15 minutes, optionally bound to one identity (credential reset).
2. The agent generates its UUID and a 256-bit credential, saves the credential as pending, and calls `POST /api/v1/agent/enroll` with token and inventory. The server consumes the token in the same transaction that inserts the agent (credential stored as SHA-256) and the host, matching an existing unassigned host by unique MAC (see Identity reconciliation). A lost response is recovered by retrying with the same pending credential.
3. `POST /api/v1/agent/report` carries `Authorization: Bearer <credential>`. The server authenticates before taking the global write lock, re-checks under it (ordering against revocation), validates the report, inserts it idempotently by `report_id`, updates the host snapshot and network membership in one transaction, and emits an event. Inventory is sent on enrollment, when it changes, and at least hourly.
4. The agent removes a report from its spool only after an accepted response and only then records the acceptance marker used by `hope-agent status`.

### Retention

Raw reports are kept for 24 hours. A 60-second timer in `main.rs` calls `metrics::retain`, which folds raw samples older than 24 hours into five-minute aggregates (per-metric sums and counts, so mixed-resolution averages preserve unavailable values) in bounded batches that release the write lock between buckets, and deletes aggregates whose whole interval is older than seven days. Late samples merge into existing rollups. History queries span both resolutions and return explicit gaps.

### SSH installation jobs

`POST /ssh/fingerprint` reads the host key without authenticating. `POST /ssh/install` creates a job bound to host id, address, port and the confirmed fingerprint, and runs the installer over SSH (`russh`). Only `hope-install: phase=`, `error=` and exact known `notice=` lines are interpreted; user-visible text comes from the catalog in `ssh.rs`. A job completes only with installer exit 0, a `complete` phase and database evidence of the new agent's accepted report newer than the replacement point. Jobs persist non-secret metadata in `install_jobs`. Details and limits: [operations.md](operations.md#installing-agents).

### Releases and manifest

`scripts/build-agent.sh` builds one target (static musl on Linux; macOS with a deployment-target check). `scripts/assemble-release.sh` combines all four platforms, signs each artifact and a `manifest.json` (version, key id, per-artifact SHA-256, size, minimum OS) with an RSA key held outside the repository, verifies the result, and publishes it by atomically swapping a `current` symlink. `releases.rs` offers a platform only if the manifest is well formed, its `key_id` matches the public key, and the files exist with the listed size. `GET /agent/install` serves `scripts/install-agent.sh` with the public key embedded. CI definitions are in `.github/workflows` (never run on GitHub yet, see `validation.md`).

## Identity reconciliation

Discovery identifies hosts by address and MAC; agents identify themselves by UUID. At enrollment (`agents::enroll_inner`) an agent is attached to an existing host only when exactly one unassigned host shares one of its MAC addresses; IP alone never matches because of DHCP reuse. Otherwise a new host is created and an administrator can merge (`POST /hosts/{id}/merge`), which keeps history and removes map positions of the merged-away record in every mode. Repeated discovery of an ambiguous MAC does not create new records. A revoked identity keeps its host; an identity-bound reset token replaces its credential. Limit: at most 250 agent identities.

## Security model

- **Administrator:** one account, created by first-run setup guarded by a token (`HOPE_SETUP_TOKEN` or a random token printed at start, stored hashed). Password hashed with Argon2; verification concurrency is capped so a flood returns 503 instead of locking anyone out.
- **Sessions:** random 256-bit token, stored as SHA-256, `hope_session` cookie `HttpOnly; SameSite=Strict`, `Secure` for an https canonical URL, 24-hour lifetime, deleted on logout.
- **Origin:** every state-changing request must match the canonical origin and must not carry a `Sec-Fetch-Site` other than `same-origin`/`none`. Agents and `curl` send neither header. Error bodies never echo the request.
- **Rate limits:** failure-only, per client, no global ceilings (a shared ceiling would let an attacker lock out the administrator); the client is the TCP peer, or the right-most untrusted `X-Forwarded-For` hop from `HOPE_TRUSTED_PROXIES`/loopback. Values in [operations.md](operations.md#request-limits-and-reverse-proxies).
- **Credentials:** agent credentials and enrollment tokens are 256-bit random values stored only as SHA-256; tokens expire in 15 minutes and are single use; credentials are separately revocable, and revocation is transactional with report acceptance.
- **Transport:** agents and the installer require HTTPS (plain HTTP only for localhost development), verify certificates, optionally against an added private CA, and do not follow redirects. TLS is terminated by the operator's reverse proxy; the server never handles certificates.
- **Headers:** CSP, `nosniff`, `no-referrer`, `X-Frame-Options: DENY`, deny-all `Permissions-Policy`, HSTS on https, `no-store` for API responses. The CSP allows inline scripts because of the Next.js export (documented limitation).
- **Signing chain:** a release key outside the repository signs the manifest and each binary. The installer script is fetched over verified HTTPS and carries the public key; it verifies manifest signature, size, SHA-256, artifact signature and version before running anything it downloaded. The key is therefore trusted exactly as far as the HTTPS bootstrap of the installer is; compromise of the server or its TLS endpoint can serve a different installer with a different key. The server itself does not verify RSA signatures. No automatic revocation of installed binaries exists beyond key rotation.
- **Process hardening:** container runs as uid 10001 with only `NET_RAW`, read-only root, bounded request bodies (256 KiB), bounded agent reports, output and timeouts on every external command, shutdown within 8 seconds.
- **Known open items:** the `rsa` advisory RUSTSEC-2023-0071 (no upstream fix, reached through russh) and a dev-tooling npm advisory (`braces`); see `validation.md`.
