# H.O.P.E implementation handoff

Audit date: 2026-10-08. Audience: an implementation model continuing this repository.

## Start here

Continue the existing implementation. Do not regenerate the app or replace its architecture. The foundation runs, but it is not release-ready. Complete the tickets below in order, keeping each change small enough to review. A passing happy-path test is not evidence that installation, discovery, or resource targets have been met.

Working directory: `/Users/martin/Developer/hope`. Current branch: `main`. Baseline commit: `1ae83e7` (`feat: build Hope discovery server, reporting agent, and web console`). Remote `origin` is now configured as `https://github.com/doomedramen/hope.git`. Stay on the current branch, commit verified milestones without co-author trailers, and push when a remote exists. Never create a branch or worktree without asking and waiting for permission. Do not reset, stash away, or discard the existing working tree.

At handoff there are uncommitted changes in protocol, server, agent, and frontend files, plus new `crates/hope-server/src/arp.rs`, `web/playwright.config.ts`, and `web/tests/`. They contain enrollment retry support, report freshness/revocation metadata, shared probe limiting, active Linux ARP, and browser tests. Preserve and finish them. This document's commit does not commit those implementation changes.

Read applicable AGENTS.md instructions first. User requests terse communication. Do not spawn agents unless explicitly authorized; if authorized, the user's default is `gpt-6-luna` with reasoning effort `max`. Work one ticket at a time. For each: inspect the named files, reproduce the defect, implement the specified behavior, run relevant checks, inspect the diff, update this document with evidence, then commit. If a required environment is unavailable, record that gate as unverified; do not replace it with a mock and claim completion.

## Fixed scope and decisions

- Independent product for up to 250 hosts. Rust Axum/Tokio/SQLx SQLite server; Rust agent; shared versioned protocol. Next.js App Router static export, shadcn/ui, Tailwind semantic tokens, shadcn/Recharts charts. Rust serves the export; production requires no Node process.
- One administrator, protected first-run setup. Canonical HTTPS server origin behind an existing reverse proxy. Custom CA support must retain certificate verification.
- Linux server host networking is the primary discovery deployment. Docker Desktop must report reduced capabilities. Linux/macOS agents, x86_64/ARM64, systemd/launchd only for the first release.
- CIDRs get fair concurrent discovery, independently bounded enrichment, cancellation, and visible progress. Agents report their own machine, not remote networks. Unanswered probes do not prove a host is offline.
- Map modes: topology, subnets, health. Show provenance for observed, inferred, and manual relationships; never fabricate physical switch connections. Preserve selection and filters across map/list changes.
- Reports every 30 seconds with jitter; inventory on enrollment, change, and periodic refresh. Explicit unavailable values, stable identity, bounded buffering, separate revocable credentials. Raw history 24 hours; five-minute aggregates through day seven.
- Manual and SSH installation share one signed installer workflow. Enrollment tokens: 15-minute expiry, one identity per token, hashed storage. Downloads precede execution. Confirm installation only after the installed service sends an accepted report.
- Defer Windows, distributed discovery, switch integrations, notifications, and multiple accounts. Do not add these while closing current gaps.

## Verified baseline and limits

The following passed against the working tree on macOS ARM64, Rust 1.96.0, Node 26.8.2/npm 11.19.1:

| Check | Result |
| --- | --- |
| `cargo test --workspace` | Six tests: one agent unit test, five server API tests |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass; dependency `russh 0.54.5` reports future incompatibility |
| `npm run typecheck` and `npm run lint` in `web` | Pass |
| `npm run build` in `web` | Static export passes |
| `npm run test:e2e` in `web` | Three Chromium tests pass: setup, seeded console/maps/history, mobile selection/reload |
| `cargo fmt --all -- --check` | **Fails**; formatting changes required |

An earlier Docker image built and served successfully, but predates the latest ARP and enrollment edits. It does not validate the current Linux code. No controlled two-range benchmark, actual service installation matrix, SSH integration test, four-platform release build, or CPU/RSS benchmark has been completed. Existing browser tests use synthetic inventory; they do not test discovery or installation. An earlier npm audit reported nine high-severity findings in transitive build tooling; rerun and resolve or document exact remaining exposure. No README or CI release workflow currently establishes a supported release process.

## Ticket 01 — Make the current tree a reliable baseline

Files: `Makefile`, `crates/hope-server/src/arp.rs`, `crates/hope-server/Cargo.toml`, Rust sources, `web/lib/protocol.ts`, frontend tooling.

1. Run `cargo fmt --all`. Keep formatting separate from behavioral fixes where practical.
2. Make type generation atomic: create a temporary file alongside `web/lib/protocol.ts`, generate into it, replace the destination only on success, clean up on failure. The existing shell redirection truncates the live file before compilation finishes. Never run frontend builds concurrently with type generation.
3. Compile and test current code on Linux, including the ARP implementation. Verify whether `pnet_datalink::IpNetwork` is exported by the selected dependency; this is a suspected Linux-only compile problem, not a confirmed one. Use the dependency's actual supported type/import.
4. Add a consistent frontend formatter and format application code so subsequent changes are reviewable. Avoid gratuitously rewriting generated shadcn components.
5. Preserve the existing browser tests and bring the currently untracked source files into Git after review.

Done when: native checks and current Linux compilation pass, a failed type-generation command cannot damage the previous file, and generated types are deterministic. Commit the completed baseline. Do not call it a release.

## Ticket 02 — Define complete contracts and database migrations

Files: `crates/hope-protocol/src/lib.rs`, `crates/hope-server/src/{schema.sql,state.rs,lib.rs}`, API handlers, `web/lib/{protocol.ts,api.ts}`.

Move all externally exchanged request/response DTOs into the shared contract, including administrator APIs currently represented by inline frontend types. Keep secrets out of bootstrap and event payloads. Define explicit enum values for scan, enrichment, agent, and install states. Use UTC timestamps with documented units. Add bounded field lengths/list sizes, finite numeric validation, sensible metric ranges, and structured API errors with stable codes.

Introduce ordered, transactional SQLite migrations. Existing databases and JSON host bodies must remain readable; new fields need defaults or a migration. Add collector and report schema-version compatibility tests. CI must regenerate types and fail on drift. Do not change types by hand in `web/lib/protocol.ts`.

Done when: an old fixture database upgrades without losing hosts/identities/history, unsupported schema versions fail clearly, malformed reports cannot create unbounded stored values, and the frontend uses the shared DTOs.

## Ticket 03 — Prove discovery fairness and finish enrichment

Files: `crates/hope-server/src/{scanner.rs,enrichment.rs,arp.rs,state.rs}`, protocol progress types, new scanner tests.

Extract a small probe interface and injectable clock so scheduling can be tested without raw sockets or real LAN traffic. Retain global concurrency and probe-start limits; account for ICMP, TCP, ARP, and enrichment under shared limits. Retain round-robin range service. Add TCP port 80 to discovery alongside 22 and 443 so an HTTP-only routed host can be found. Define and test connection-refused handling: a remote TCP refusal is liveness evidence, not an open service. Keep timeout, unreachable, permission failure, and cancellation distinct.

Replace enrichment's `try_acquire_owned` drop behavior with a bounded, deduplicated work queue. Excess responsive hosts must eventually receive enrichment. Use fair processing and bounded DNS/service attempts; discovery completion must not wait for enrichment. Expose pending, completed, and failed enrichment state. Use a cross-platform bounded DNS mechanism; `getent` alone does not support macOS. A cancelled scan must not continue launching scan-owned work; shared host enrichment ownership must be explicit.

For ARP, choose the source IPv4 address from the interface network containing the target. Validate frame length, Ethernet/ARP protocol fields, opcode, source/target addresses, and outstanding request before accepting a reply. Do not infer liveness from stale neighbor-cache entries. Move potentially blocking raw-channel writes off Tokio workers. Bound pending requests and thread/channel lifecycle; report actual runtime capability loss.

Tests required:

- Two `/24` ranges submitted close together both receive work before the first finishes, including one entirely silent range.
- Slow DNS, blocked ports, HTTP-only hosts, unavailable raw-socket privileges, overlapping ranges, cancellation while queued/in flight, and restart interruption.
- Global observed concurrency and starts per time window never exceed configured limits; enrichment cannot starve discovery or vice versa.
- ARP malformed/truncated/unsolicited/wrong-target frames, secondary interface addresses, timeout cleanup, and missing capabilities.
- At least 250 responsive hosts eventually get enrichment; queue saturation remains bounded and visible.

Then build an isolated Linux network-namespace/container fixture with two routed `/24`s, documented responders and blackholes. Measure first publication and discovery completion over at least five runs. Targets: first responsive host within five seconds and both discovery passes within 30 seconds. Record actual settings, packet count/rate, hardware, topology, and individual timings. Do not scan the user's real networks as a substitute. If targets fail, profile scheduling and deadlines before increasing traffic limits.

## Ticket 04 — Make identities, reports, and retention correct under failure

Files: `crates/hope-server/src/{agents.rs,metrics.rs,state.rs}`, merge/map handlers, protocol, API tests.

Finish the pending-credential enrollment design already present: the agent persists a random credential before enrollment; the server stores its hash. Retrying a lost enrollment response with the same identity and credential may recover the same enrollment without creating a second host. A different identity or credential cannot reuse the token. Expiry remains enforced. Revoked credentials cannot use this path to resurrect themselves.

Check report authorization and revocation consistently with the write transaction so a request waiting behind revocation cannot subsequently commit. Keep duplicate report IDs idempotent. Older samples may enter history within the permitted window but must not replace latest inventory/metrics. Commit host association/network membership and sample storage atomically.

Only associate hosts automatically using unique, strong evidence such as a unique matching MAC. An IP match alone cannot transfer an agent identity. Handle conflicting MAC/IP observations without creating endless duplicate hosts. Manual reconciliation must preserve history, rewrite links, and remove all qualified map position IDs for the merged host, including `${mode}:${id}` keys.

Retention must be restart-safe and idempotent. Preserve per-metric counts/sums or equivalent sufficient statistics so averages of aggregates are weighted correctly, including partially available metrics and raw/rollup boundaries. Missing data must stay missing. Prevent a late report from replacing a complete aggregate with a partial bucket. Process cleanup in bounded batches that release the global write lock between batches.

Tests: lost enrollment response/retry, concurrent token reuse, credential mismatch, revoke/report race, DHCP change, cloned identity behavior, ambiguous association/manual merge, out-of-order and duplicate reports, null-only buckets, weighted mixed-resolution data, exact 24-hour/seven-day boundaries, repeated cleanup, restart during cleanup. Benchmark retention with seven days of 250-host data while reports and discovery writes continue.

## Ticket 05 — Bound agent resource use and delivery behavior

Files: `crates/hope-agent/src/{main.rs,config.rs,collect.rs}` and focused agent tests.

Separate collection cadence from network draining, or give draining a strict time budget so ten slow HTTP requests cannot delay sampling by 100 seconds. Keep a single bounded spool with explicit count, byte, and age limits at runtime and startup. Retry transport errors, 429, and server errors with bounded exponential backoff/jitter; honor bounded Retry-After. Surface revocation distinctly and stop hot retries. A dropped invalid report must never count as an accepted report or successful enrollment.

Add exclusive process ownership for each agent state directory. Persist identity, pending credential, spool, and acceptance markers atomically with restrictive permissions; use unique temporary files and avoid simultaneous writers. Preserve identity during reinstall/upgrade. Define certificate-path updates explicitly rather than silently ignoring new CA settings when configuration already exists. Handle SIGTERM and SIGINT with a bounded shutdown.

Bound collector command stdout/stderr while reading, not after `Command::output()` has allocated it. Kill and reap timed-out/oversized subprocesses. Cache inventory work, sort inventory before fingerprinting, and keep expensive collectors off the reporting critical path. Document whether network totals include virtual interfaces and avoid double-counting stacked disks. Expose per-interface counters if needed to satisfy networking inventory/metrics, while retaining explicit reset/unavailable semantics. Missing GPU interfaces must produce nulls and availability reasons.

Tests: offline startup, sustained outage, full/corrupt spool, 429, TLS failure, credential revocation, clock skew, counter reset, hanging/noisy collector, duplicate process, interrupted atomic writes, inventory changes, unchanged inventory suppression, and actual accepted-report success tracking.

Done when: bounded behavior is demonstrated under faults, no secret appears in logs, and normal sampling remains close to 30 seconds during a slow/disconnected server. Resource targets are measured in Ticket 11, not inferred from language choice.

## Ticket 06 — Finish map, detail, and history behavior

Files: `web/components/{console.tsx,network-map.tsx,metric-history.tsx,host-details.tsx,install-dialog.tsx}`, `web/lib/api.ts`, server map handlers, Playwright tests.

- Key temporary drag positions by mode and host, matching persisted positions. Restore each mode independently. Fit the viewport on explicit mode changes/first useful dataset, without fighting the user's later pan/zoom.
- Define grouping for overlapping networks; a filtered host must not disappear because its first network differs from the selected network. Keep manual/observed relationships visually and textually distinct.
- Replace stale whole-map overwrites with revision-checked writes or atomic position/link operations. Prefer atomic operations for independent edits. Surface save failures and preserve/retry unsaved edits without losing unrelated changes.
- Preserve selection and filters through view switches; maintain selected host via query parameter and browser back/forward. Provide an empty filtered state and a clear stale/revoked/unobserved distinction.
- Remove all null-to-zero conversions for missing memory and other metrics. Draw explicit leading, internal, and trailing history gaps through the selected time window. Label dates for multi-day charts. Include useful disk/GPU history when available, with clear unavailable states otherwise.
- Handle API/session expiry centrally, close SSE, and return to login. Do not leave a permanent erroring dashboard. Make logout reachable on mobile.
- Separate network creation from scan retry so a failed initial scan does not lead users to create the same range again. Retrieve latest progress for every network independently of a global last-100-jobs limit.
- Clear SSH secrets when the dialog closes or a job starts. Explain unavailable release artifacts and custom CA requirements before offering a command that cannot run.

Acceptance: extend Playwright for all three modes, per-mode saved positions after reload, simultaneous map edits, filter/selection/back navigation, missing metrics and chart gaps, session expiry, failed scan retry, merge cleanup, and mobile logout. Inspect desktop and mobile screenshots manually. Verify keyboard focus, labels, readable status distinctions, no horizontal overflow, and consistent semantic Tailwind tokens. Existing three browser tests are a starting point only.

## Ticket 07 — Make installation transactional and service-confirmed

Files: `scripts/install-agent.sh`, `crates/hope-server/src/{agents.rs,releases.rs}`, agent status/acceptance handling; new installer test harness.

Use an explicit preflight → stage → verify → enroll/configure → activate → service-report-confirmed sequence. Preflight resolves OS, architecture, service manager availability, existing install mode, writable paths, root/sudo capability, HTTPS origin, CA access, and release availability before stopping a healthy service. Detect a running systemd manager, not just a `systemctl` binary. For noninteractive sudo failure, fall back to a supported user service with a clear explanation; never hang waiting for a password.

Honor explicit user/system mode and detect existing mode on upgrade/uninstall. Root execution must not accidentally uninstall a different system installation when targeting a user installation. Explain systemd user lingering and macOS GUI-session limits. Keep identity by default on uninstall; document an explicit purge option only if implemented.

Stage binary and service definition; verify the signature before executing any downloaded binary. Keep the previous binary/config/service definition until activation succeeds. On failure or interruption, restore them and restart the previously healthy service. Re-running after interruption must converge safely. Use dedicated service users/groups; do not place the macOS daemon in the shared staff group. Validate pre-existing service-account identity before reusing it.

A foreground enrollment report does not prove daemon health. Capture the activation boundary and wait for an acceptance marker tied to a report produced by the installed service after activation. Only then print success. Never print enrollment tokens or credentials. Preserve TLS verification for installer, artifact, enrollment, and report requests. Parse embedded signing keys robustly; document that the installer/signing key bootstrap is trusted through HTTPS.

Harness tests must cover root, working sudo, unavailable/denied sudo, user service, unsupported manager/architecture, existing identity, malformed URL, missing CA, bad signature, download failure, token expiry/reuse, failed service start, failed first report, reinstall, upgrade, uninstall, and signals during each mutation phase. Use temporary filesystem roots and command adapters, not the developer's actual service manager. Then validate real services in the matrix in Ticket 10.

## Ticket 08 — Complete SSH installation jobs

Files: `crates/hope-server/src/ssh.rs`, installer status protocol, `web/components/install-dialog.tsx`, new SSH fixture tests.

Keep host-key discovery separate from trust confirmation. A mismatch must fail before sending authentication material. Bind the job to the selected host/address/port and the confirmed fingerprint. Retain bounded job concurrency and lifetime. Credentials and decrypted keys remain only in memory and are dropped after use; never persist raw remote output.

Replace generic discarded-output failures with a small allowlisted installer progress protocol: stage, stable error code, and vetted user-facing message. Treat remote output as untrusted. Ensure secrets cannot enter logs through malformed output. Align inactivity and total timeouts so a legitimate quiet download does not fail after 30 seconds. On connection loss/timeout, report uncertain remote state unless termination is confirmed; do not imply rollback occurred remotely.

Fix reinstall confirmation: an existing agent may retain its credential and not consume the job's new enrollment token. Determine success through the installer's bound existing identity and post-activation accepted report, not token consumption alone. Persist only nonsecret job metadata and mark unfinished jobs interrupted after restart. Let the UI reopen job progress/history and offer actionable retry guidance.

Tests with an isolated SSH server: verified key success, changed key, wrong password/key/passphrase, denied sudo/user fallback, custom CA, quiet operation, progress errors, dropped connection, restart, same-agent reinstall, unavailable artifacts, and first-report timeout. Assert secrets absent from database and captured logs. Do not install onto a user's selected machine as an automated test.

## Ticket 09 — Operational hardening and deployment

Files: server lifecycle/auth handlers, `Dockerfile`, `compose.yaml`, `compose.desktop.yaml`, dependency manifests.

Handle SIGTERM as well as SIGINT. Stop scheduling, cancel/settle work, close SSE and bounded background tasks, and persist interruption state within Docker's stop grace period. Keep SQLite data persistent and document backup/restore; use a consistent SQLite backup strategy rather than copying a live main file without its WAL.

Apply appropriate no-store/security headers to authentication, enrollment, and secret-bearing responses. Test secure cookies, same-origin mutation protection, session expiry/logout, bounded login/report/enrollment abuse, SSE reconnect/lag, and reverse-proxy disconnects. Bound error messages and avoid secret reflection. Inspect all rate limits for global denial-of-service side effects.

Rebuild the latest Linux image. Verify capabilities match actual container privileges, Linux host-network discovery works in the controlled fixture, and Desktop limitations are explicit in the UI. Validate Compose configuration, persistent restart, HTTPS proxy operation, custom CA, and health checks. Review npm audit and Rust dependency advisories; fix compatible vulnerabilities and inspect `russh`'s future-incompatibility warning. Do not blindly force a breaking dependency downgrade.

## Ticket 10 — Signed release matrix and continuous integration

Files: `scripts/package-agent.sh`, new release manifest/build scripts, `.github/workflows/`, release-serving code, installer fixtures.

Build all four agent targets on suitable runners: Linux x86_64/ARM64 and macOS x86_64/ARM64. Declare minimum OS/libc versions based on actual artifacts. Use one controlled release signing key, kept outside Git and images. Prevent publishing a new public key alongside old artifacts signed by another key. Publish a signed version/platform/checksum manifest or equivalent verifiable metadata so artifacts cannot silently mix versions. Publish atomically only when every required artifact/signature is present and verified.

CI gates: formatting, Clippy, Rust tests on Linux/macOS, protocol drift, frontend typecheck/lint/export, browser acceptance, shell syntax/ShellCheck and installer harness, Linux discovery fixture, and artifact signature verification. Run privileged/service tests only in disposable trusted runners. Protect signing credentials from untrusted pull-request code.

Real installation matrix: each OS/architecture, system service and supported user service, admin and no-admin, default and custom CA. Exercise reinstall/upgrade/uninstall and interrupted activation with old service recovery. Record actual runner OS, architecture, service manager, artifact version/hash, and results. Cross-compilation alone does not satisfy installation validation. Explicitly mark unavailable matrix cells unverified and block claiming full platform support.

## Ticket 11 — Benchmarks and operator documentation

Create `README.md`, `docs/architecture.md`, `docs/operations.md`, `docs/validation.md`, and repeatable benchmark scripts as needed. Check repository licensing intent before adding a license; the Cargo manifest alone is not a license grant.

Measure release agents on documented Linux and macOS reference machines, with and without supported GPU collectors. Measure idle baseline and at least 30 minutes of ordinary reporting after warmup. Report RSS median/peak, CPU time divided by wall time as a percentage of one core, bytes transferred, collector duration distributions, inventory refresh cost, and disconnected-server behavior. Keep measurement overhead separate. Targets: below 30 MB resident memory and average CPU below 0.5% of one core during normal reporting. The current `benchmark` command only measures sample duration and cannot prove these targets. Publish failures honestly and optimize the measured expensive path.

Document installation, canonical URL/proxy setup, protected first-run token, custom CA placement on server versus agent/SSH destination, monitoring versus initial discovery, capability limitations, probe traffic, identity reconciliation, signing/rotation, revocation, upgrade/rollback/uninstall, backup/restore, and troubleshooting by stable error code. Include measured discovery timings and agent costs, exact supported platforms, and remaining limitations. Never publish a performance claim without the corresponding reproducible evidence.

## Final verification and release gates

Run type generation first, then checks; do not overlap generation and frontend builds. After Ticket 01 fixes atomic generation, the repository-level sequence is:

```sh
make types
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --locked
cargo build --locked -p hope-server
npm --prefix web ci
npm --prefix web run typecheck
npm --prefix web run lint
npm --prefix web run build
npm --prefix web run test:e2e
docker compose config
```

Also run the new Linux discovery, installer, SSH, migration, retention, security, and release-matrix suites introduced above. Verify generated files stay unchanged, examine `git diff --check`, and build/test the actual release image and signed artifacts. Browser tests require Playwright's browser installation; install it explicitly in clean CI. Never confuse the old `hope:local` smoke image with the current build.

A release is complete only when all five product stages are evidenced: (1) discovery/hosts, (2) map modes, (3) agent/history, (4) manual installation, (5) SSH/release hardening. Passing local tests alone does not complete any environment-dependent gate. Keep a table of commands, environment, date, artifact commit/hash, results, and links to logs/screenshots/measurements in `docs/validation.md`.

Before each milestone commit, review staged files and ensure no keys, tokens, databases, binaries, test artifacts, or unrelated changes are included. Commit on the current branch, without co-author trailers. Push if a configured remote is available. If not, state that plainly and continue local work.

## Suggested next-model prompt

> Continue H.O.P.E from the current branch and working tree. Read AGENTS.md and docs/IMPLEMENTATION_HANDOFF.md. Start with the first incomplete ticket, preserve all existing changes, and execute tickets in order. Follow the behavior and acceptance criteria exactly; add focused tests for failure cases. Update the handoff with evidence and commit verified milestones without co-author trailers. Do not claim unrun platform tests or benchmarks passed. Stay on this branch; do not create worktrees. Keep going through all locally achievable work and report external validation gaps precisely.

## Implementation progress

### Ticket 01 — completed 2026-10-08

- Preserved and reviewed the previous working tree; formatted Rust and application frontend files, added Prettier commands.
- Type generation now stages a sibling temporary file and replaces the contract only on success. Injected a failing Cargo command and verified byte-for-byte preservation and temporary-file cleanup.
- Linux compilation exposed the private `pnet_datalink::IpNetwork` import; replaced it with the public address accessor. Linux ARM64 release workspace compilation passed in the Docker Rust stage.
- Native Rust tests (six), Clippy, formatting, frontend typecheck/lint/static export, and three Chromium tests pass. Generated protocol trailing whitespace was removed at the source generator.
- This establishes a build baseline only. Discovery, installation, and resource gates listed above remain open.

### Ticket 02 — contract and migration foundation completed 2026-10-08

Shared DTOs now cover authentication, networks, merge, metric windows, enrollment tokens/reports, and SSH requests/responses. Scan/install states use Rust enums and generated TypeScript unions. Errors include stable status-based codes. Inventory, collector metadata, and metric dimensions have bounded strings/lists and finite numeric/range checks. Database adoption uses an ordered transactional version, preserves legacy identities/history, rejects future versions, and remains idempotent. Tests cover legacy host defaults, status values, invalid reports without storage, and database adoption. Native tests/Clippy and frontend types/lint pass. CI contract-drift enforcement is scheduled in Ticket 10.

### Ticket 03 — discovery/enrichment milestone completed 2026-10-08

The scheduler has an injectable probe backend and bounded host deadlines. TCP port 80 and refused-connection liveness are supported. Enrichment uses a bounded deduplicated queue, eight workers, visible host states, killable Linux/macOS resolvers, and capped resolver output. ARP validates frame fields, correlates source/target requests, chooses matching interface addresses, bounds pending requests, and moves raw sends off Tokio workers. Cancellation is rechecked after obtaining the persistence lock. Latest scan records remain available for each range.

Native and Linux ARM64 tests/Clippy/formatting pass. Ten controlled discovery runs passed: five routed runs completed both ranges in 25.397–25.437 seconds; five on-link runs in 26.392–26.464 seconds. First responders appeared in 0.772–0.817 seconds. See docs/validation.md and its raw benchmark files. The shared limit is 200 probe starts per second; an absolute interface packet policer and measured frame counts are not claimed.

### Pause checkpoint — Ticket 04 partially completed, 2026-10-08

The user requested a pause. Resume at Ticket 04; do not skip to release claims.

Verified changes:

- Enrollment retries recover a lost response without duplicating identity; concurrent reuse permits only one identity.
- Report authorization occurs under the same write lock as revocation. A queued report cannot commit after revocation. Credential and visible revocation state change transactionally.
- Revoked identities can retain their host/history through an administrator-issued token bound to that identity and a new credential. Old credentials remain rejected; retrying the replacement is idempotent. The installer command and frontend support this token scope, and sudo preserves the explicit reset flag.
- Reports commit network membership with host/sample state. Manual merges remove qualified source positions in every map mode. Ambiguous discovery observations no longer create a fresh duplicate on every scan.
- Migration 2 adds rollup statistics, bound reset-token metadata, and cleanup indexes. Rollups keep per-metric sums/counts; mixed-resolution averages preserve nulls and use those counts. Repeated retention merges late samples with existing rollups instead of replacing a full bucket. Each bucket commits and releases the write lock; cleanup is bounded.
- Legacy rollups lack recoverable counts. They remain unchanged and bypass further display grouping until expiry. Exceptional legacy late raw samples are preserved until seven-day expiry and excluded from new rollup work so they cannot starve normal cleanup.

Checks at checkpoint: 24 native Rust tests, Clippy, Rust formatting, shell syntax, frontend typecheck/lint, and static production export pass. Latest Linux image and discovery benchmark evidence belong to the preceding discovery milestone; Ticket 04 has not yet been rebuilt/retested on Linux or exercised in browser acceptance.

Still required for Ticket 04: seven-day/250-host retention load while reports/discovery writes proceed; explicit boundary/restart tests; ambiguous MAC and manual-merge regression tests; full review of association behavior. Before extending GPU history, add stable GPU identifiers: vendor/name keys collide when a host has multiple identical GPU models, and collector correlation needs the same identifier. Tickets 05–11 remain open. Visible browser review is also pending: the user asked to see the site when interactive testing starts; open it visibly in Codex before that testing.

### Ticket 04 — completed 2026-10-09

Finished deterministic retention boundary/restart tests, unique/ambiguous MAC enrollment, IP-only refusal, manual merge/history/map preservation, and repeated ambiguous-discovery tests. Fixed premature expiry of partial seven-day buckets and included overlap in history queries. Native and Linux ARM64 formatting/Clippy and 28 tests pass. Explicit release load tests on both platforms use 1,154,500 samples for 250 hosts; cleanup takes 0.226 seconds natively and 0.552 seconds in the Linux VM, while report-handler maximum latency stays below 15 ms. See docs/validation.md for scope, commands, environment, and raw results.

The macOS release build also required unstripped build-time proc macros; shipped binaries still use release stripping. Removed unused SQLx default features. OrbStack BuildKit's default DNS failed repeatedly; a validation-only `--network=host` build completed Linux checks and the load test. Tool installation now precedes source copying so changes can reuse that layer.

Remote origin is configured. Continue with Ticket 05 and push verified milestones on main. GPU identifiers and visible browser review remain required.

### Ticket 05 — delivery and collector milestone, 2026-10-09

Collection now races in-flight HTTP requests under a single spool owner. Runtime/startup spool bounds are 120 reports, 4 MiB, and one hour; corrupt/oversized files, excessive decoded counts, wrong identities, and future timestamps are rejected or evicted. Retry-After (seconds/date), transport/429/server failures, bounded backoff, explicit revocation, and accepted-only success markers are implemented. Inventory suppression advances only after acceptance and cannot regress on older backlog. State writes are atomic/private with an exclusive process lock. CA updates validate PEM bundles; redirects remain disabled; SIGTERM/SIGINT save the spool.

Commands cap output while reading and kill/reap on deadline. Filesystem probing permits only one detached worker, bounded waiting, cached inventory, and explicit unavailable metrics. GPU IDs distinguish identical cards where native interfaces expose identity; unidentified legacy devices retain snapshots instead of ambiguous averages. Per-interface lifetime counters/rates are included, with reset/unavailable semantics and explicit total membership; default-route totals avoid common virtual-interface duplication. Stacked disk counters are excluded.

Native and Linux ARM64 Rust formatting, Clippy, and 42 ordinary tests pass, including 14 agent fault tests. Frontend types and production export pass. Actual 30-second cadence/resource measurements and platform certificate/service tests remain gates in Tickets 10/11. No CPU/RSS target is claimed from these tests. Visible local preview opened before interactive testing, with separate demo state and disabled 198.18.x discovery ranges. Continue Ticket 06.

### Ticket 06 — console/map/history milestone completed 2026-10-09

Map edits use atomic operations under the server write lock, monotonic JSON-safe revisions, and revision-checked whole-map replacements. Concurrent independent edits survive; conflicting link retries and stale replacements fail explicitly. The client queues edits, retains failures for retry, and rejects stale refresh/save responses. Drag positions are mode-qualified; each mode fits once after controlled-node dimensions arrive. Larger groups use wider grids and a lower overview zoom floor. Overlapping hosts choose the first visible configured network; filtering to one range groups its members there. Unassigned hosts do not acquire invented subnet edges, and group headers sit clear of gateway paths.

Host selection uses browser history; filters/selection survive map/list changes. Session expiry closes the mounted dashboard/SSE and returns to login; logout is reachable on mobile. Missing memory stays unavailable. History covers the full selected window with leading/internal/trailing gaps, multi-day date labels, selected identified GPU and filesystem charts, and explicit missing states. Details include per-interface counters and total scope, collector/enrichment state, and cached storage inventory when current filesystem metrics are missing. A failed initial scan retries its saved network. Installation dialog secrets are discarded on unmount; package availability is shown before commands are enabled. Package presence is not signature verification; the installer verifies signatures, and Ticket 10 must harden release metadata/serving.

Native/Linux ARM64 formatting, Clippy, and 43 Rust tests pass. Frontend formatting/types/lint/static export and nine Chromium cases pass: setup/missing packages, map modes/manual-link removal/null memory, mobile reload/logout, actual drag with simulated save failure/retry and per-mode reload, back navigation/session expiry, overlapping filters, failed scan retry, and history gaps/boundaries. Desktop/mobile screenshots were inspected after waiting for measured viewport fitting. Browser preview was shown visibly before testing. Its existing records and subsequent externally added range/selection were preserved; automated tests use a separate fixture server/database. Continue Ticket 07; service installation, SSH, release matrix, operations, and resource benchmarks remain open.
