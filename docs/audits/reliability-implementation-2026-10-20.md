# Reliability implementation — 20 October 2026

Scope: Linux x86_64 only, as confirmed by the project owner. This ledger follows the [baseline review](reliability-review-2026-10-20.md). It records local implementation and verification; it does not certify a deployed release.

## Implemented

| Review finding | Change | Regression evidence |
| --- | --- | --- |
| 1 — Stored records and derived device views share a persistence path | `StoredHost` is a distinct validated storage type. Mutation paths load stored records; projected views cannot be passed to `save_host`. Source overrides live in their own table. Identity decisions run through one transactional reconciler, with unique indexes enforcing confirmed associations. Migration 15 repairs legacy persisted projections while preserving history and aliases. | Compile-fail boundary test; network refresh, settings/report, migration and association constraint tests. Existing source migration/merge and scanner tests still pass. |
| 2 — Unlink immediately relinks | Unlink records a durable manual rejection; explicit reset permits automatic reconciliation again. Reads no longer make identity decisions. | Unlink followed by reads, a report, and explicit reset; database pointer assertions before projection. |
| 3 — Artifact overlays lose checksum entries | Publishing downloads into separate staging directories, validates the expected server artifact and generates the final manifest from the final server/agent artifacts. Scope follows the concurrent Linux x86_64 restriction. | Release assembly tests reject absent, corrupt and duplicate manifest entries and a missing agent bundle. |
| 4 — Failed server upgrades destroy the working installation | Host and Proxmox upgrades use one activation helper with a writer lock, stopped-service verification, private SQLite/file backups, durable journal, rollback and an orphaned-activation startup guard. Health must come from the unit's process ID. The old source checkout stays intact; agent signing and the Proxmox version marker advance only after activation succeeds. | Real SQLite/filesystem fault injection with fake systemd/HTTP covers migration rollback, WAL contents, wrong-process health, failed first install, stop failure, signals, retry, concurrent installers, custom configuration and the Proxmox binary layout. Host integration tests verify preservation of the signed agent release after failed activation. |
| 5 — Transient notification failures lose messages | Migration 16 adds a bounded durable outbox. Condition transitions and messages commit together; failed HTTP requests retain messages with persisted retry deadlines. Endpoint and credentials are snapshotted together. | Failed publish, restart, retry, success and recovery ordering; queue-capacity deferral; settings cancellation. |
| 6 — Notification HTTP blocks discovery | Independent condition evaluator and delivery worker; four concurrent deliveries, per-condition ordering, bounded HTTP deadline and cancellation-aware shutdown. Network I/O holds no database write lock. | A deliberately stalled endpoint allows evaluation to complete and graceful shutdown to retain both queued transitions. |
| 8 — Live events remove source data | Agent reports publish the same canonical projection used by GET/bootstrap. The browser tracks events that arrive during a snapshot request without relying on the agent's clock. | Real report/source/snapshot/SSE API test and Chromium test checking that source details remain visible through live reports. |
| 9 — Hanging browser requests stop refresh | API requests have a 20-second deadline covering headers and body, plus caller cancellation. `useInventory` owns SSE, snapshot recovery and coalesced refreshes. Requests queued during a fetch trigger a subsequent fetch. Source history refreshes every 30 seconds and retries after failures; host history periodically reloads even without new reports. | Transport tests for stalled connections, stalled bodies and caller cancellation; Chromium tests for snapshot recovery, authentication retry and source history refresh. |

Final verification also exposed an intermittent SQLite lock error when a host save raced SSH progress. Read/modify/write transactions now reserve SQLite's writer with `BEGIN IMMEDIATE` before reading; read-only projections retain ordinary snapshot transactions. This avoids stale-snapshot write upgrades even when an independent single-statement writer does not use the application mutex. A deterministic test holds an independent writer, verifies that a host save waits, and then confirms successful persistence after the writer commits.

The storage migration cannot infer the provenance of addresses already unioned by old versions. Those addresses are conservatively retained. Existing IDs, source histories and aliases remain reachable. The identity refactor does not add retire/forget/purge semantics.

Notification delivery is at least once, so crash recovery can duplicate an accepted message. The queue is bounded at 4,096 entries. Full queues postpone condition changes; new-device announcements at capacity are omitted with a warning. These limits and cancellation semantics are documented in [operations](../operations.md#notifications).

## Validation

- Rust workspace: 263 passed, one intentionally ignored retention load test, including the storage-boundary compile-fail doctest.
- Strict workspace Clippy and Rust formatting pass.
- Web production build, TypeScript and ESLint pass.
- Chromium/Playwright: 62 passed, including five new transport/recovery tests and extended real-server live/source history tests.
- Release assembly harness: 19 passed.
- Installer harness: 162 run; 149 passed and 13 out-of-scope macOS agent fixtures skipped. The host fixtures now stub the operating system explicitly and run on this macOS development host; 16 activation tests use real SQLite and filesystem operations with only systemd/HTTP stubbed.
- Shell syntax, ShellCheck, generated protocol drift and eight documentation tests pass.

The Rust, installer, release, shell and documentation checks were rerun for server activation. The browser suite covers the preceding inventory/transport changes; activation changes the public health response only by adding its process ID. Linux service-manager, raw-socket and pmxcfs behavior, actual GitHub publication, populated load and multi-day soak remain unverified. Docker is installed here but its daemon is unavailable.

## Remaining blockers before recommendation

1. **Clock-independent report ordering (finding 7).** Add durable ordering with explicit restart/backlog behavior, expose observed versus received freshness and test forward/backward clock corrections. The currently accepted future-timestamp freeze remains reproducible; changing live state on every receipt would regress backlog protection.
2. **Pinned installation artifacts (finding 10).** Select an immutable release and keep manifest, signatures, installer and binary pinned across all downloads. Test publication between each download step, and retain referenced releases for a defined installation window.
3. **Linux x86_64 acceptance.** Exercise actual packaged artifacts through fresh installation, upgrade, failed activation/restore, restart, interface replacement/raw-socket recovery, notification outage, database/disk pressure, guest churn and browser reconnection. Measure realistic inventory/history load and run a multi-day soak. The locally passing suites do not establish these release gates.

Additional follow-ups from the baseline review remain: discovery interface recovery, deliberate device retirement and agent-cap handling, monitoring-system health/readiness, queue/retention lag visibility and realistic populated-load measurements. These are not claimed as completed.

The project is materially more reliable, but the remaining clock and release-selection defects, and the unverified Linux activation gate, still prevent a recommendation for unattended use.
