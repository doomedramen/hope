# Reliability and refactoring review — 20 October 2026

Reviewed commit: `e564e16` (clean working tree at the start).

This is the baseline review. Subsequent implementation and remaining release gates are tracked in [the implementation ledger](reliability-implementation-2026-10-20.md). The supported release scope was subsequently narrowed to Linux x86_64 only.

**Decision: retain the Rust/Axum/SQLite server, Rust agent and static console. Do not recommend this release for unattended use yet. Rewrite the device-identity orchestration and server upgrade transaction; replace notification delivery with a durable outbox; make focused corrections elsewhere. A whole-project rewrite would discard useful reliability work without addressing the actual failure modes.**

This was a risk-based code review, with executable diagnostics for selected defects, rather than a claim that every line or platform has been certified. Existing audit ledgers were background context; their unfinished coverage was not treated as completed review. No production code was changed and no external installation was modified.

## Release blockers, in priority order

### 1. P1 — Derived device views can be written back as authoritative records

Evidence: `crates/hope-server/src/state.rs:298` (`hosts`), `:389` (`host`), `:436` (`save_host`), `:607` (`merge_source_host`); `crates/hope-server/src/scanner.rs:418`; `crates/hope-server/src/discovery.rs:148`.

`State::hosts()` joins stored hosts, source devices, relationships and associations into a mutable `Host`. It also makes additional identity decisions while reading. Scanner persistence and periodic classification then pass those projected objects to `save_host`, which accepts exactly the same type as authoritative stored records. Source fields and combined addresses can consequently become embedded in `hosts.body`.

**Reproduced:** create a scan host and matching child source, project the combined host, save it using the production projection/save pattern, then reject the association. Inventory contains **two displayed rows carrying the same source-device ID**. The saved scan host retains the old child/source fields while the now-unlinked source is projected separately. The diagnostic exercises the component call pattern, not a physical LAN scan.

This is the clearest architectural problem in the project. It permits stale state, duplicate devices, and history/navigation disagreements to recur despite individual reconciliation patches.

**Change:** introduce separate persisted host/source records and a read-only device-view DTO. Persist scan observations, agent observations, user overrides and identity associations through dedicated operations. Build views from those records; prohibit saving a view. Move association decisions into one transactional identity service, with database constraints for the one-to-one relationships the application intends to enforce. Preserve source provenance, incarnation and old URL aliases.

**Acceptance:** after scan/report/classify/restart/link/reject/unlink/merge sequences, each canonical identity appears once, overrides survive, detached source data disappears from its former target, and history remains reachable. Test sequences and arrival-order permutations, not just individual endpoints.

### 2. P1 — Unlink succeeds, then the next inventory read silently relinks

Evidence: `crates/hope-server/src/agents.rs:1234`; `crates/hope-server/src/state.rs:347`, `:448`.

The unlink action deletes the association. Read-time fallback then finds the same strong interface match and presents it as confirmed again, without persisting that decision. The UI exposes an explicit Unlink action, so this is observable broken behavior.

**Reproduced:** confirmed association → successful unlink response with no child → immediate `State::hosts()` returns the child attached again with a confirmed association.

**Change:** define durable manual intent. Unlink must suppress automatic reassociation until an explicit reset or a defined change in identity evidence. Read operations must not invent a confirmation that disagrees with stored state. Address this inside the identity rewrite above, rather than adding another projection exception.

### 3. P1 — Published server checksums can omit one architecture

Evidence: `scripts/package-server.sh:57`; `.github/workflows/server-build.yml:115`, `:137`; `.github/workflows/release.yml:93`, `:100`; `scripts/install-hope.sh:211`.

Each architecture artifact contains a file named `SHA256SUMS-server`. Both publishing workflows download these artifacts into the same directory using `merge-multiple: true`. They then sort the single resulting checksum file. Overlaying identically named files cannot concatenate their contents: one architecture's checksum can be lost. `sha256sum -c` verifies only listed files, so the publishing check does not detect the omission. The host installer refuses the unlisted architecture even though its archive exists.

**Evidence level:** code-traced packaging defect; GitHub Actions was not executed during this review.

**Change:** download into separate architecture directories and generate the final checksums from the complete final artifact set. Assert that both server archives and the agent bundle are listed exactly once. Exercise the published layout through both architecture installer fixtures.

### 4. P1 — Server upgrades can destroy the working installation before replacement is proven

Evidence: `scripts/install-hope.sh:454`, `:460`, `:468`, `:477`; `ct/hope.sh:66`, `:78`, `:114`; `crates/hope-server/src/migrations.rs:9`.

The host installer replaces the server executable and deletes the previous web export before validating the replacement service. A failed health check only warns, followed by “H.O.P.E is installed.” Database migrations run when the replacement starts, without an installer-created pre-upgrade backup. The Proxmox upgrade path stops the service and can remove the prior source installation before fetching its replacement.

A failed download, startup error, incompatible package, interruption, or migration failure can leave a user with no working server and no automatic recovery. This is substantially weaker than the transactional agent installer.

**Change:** use immutable versioned server directories and one activation pointer. Download and validate everything first; back up SQLite consistently; retain the old executable, web export, configuration and database recovery point. Stop only for activation. Confirm readiness and the activated version, then complete the transaction. Restore the old installation on failure; explicitly handle schema compatibility when choosing between binary rollback and database restoration. Do not report success after failed readiness.

**Acceptance:** real systemd tests for failed downloads, invalid executable, migration failure, full disk, interruption at each activation step and restart after power loss. The previous usable version must remain recoverable, including its database.

### 5. P1 — Alert delivery is lost after a transient failure

Evidence: `crates/hope-server/src/notify.rs:468`, `:511`, `:528`, `:553`.

The evaluator inserts an open alert before sending. If ntfy fails, subsequent evaluations skip the already-open alert. Recovery deletes the alert before sending and has the same loss problem. Logs and a last-error field do not provide delivery recovery.

**Reproduced:** a local ntfy fixture returns HTTP 503. Three evaluations of the same persistent offline condition produce **one HTTP request**, with no retry.

**Change:** separate condition state from delivery state. In one transaction, record the alert transition and a bounded outbox entry. A delivery worker retries transient errors with backoff, records delivery outcomes, and preserves pending transitions across restart. Define ordering and coalescing for open/recovery events. Preserve the existing policy that enabling notifications does not replay every historical problem. Expect at-least-once delivery; a crash around remote acceptance may produce a duplicate.

### 6. P1 — Notification and discovery scheduling share one slow serial loop

Evidence: `crates/hope-server/src/discovery.rs:31`, `:110`; `crates/hope-server/src/notify.rs:468`; `crates/hope-server/src/state.rs:208`; `crates/hope-server/src/uptime.rs:97`.

Scheduled scans, multicast discovery, sequential routed traces, classification, TCP uptime checks and serial notification delivery run in one task. A slow notification endpoint or a large outage delays the next scan scheduling and the next alert evaluation. The configured notification timeout is ten seconds: 250 newly-open alerts that each time out imply roughly **2,500 seconds of serial delivery waits**, before other work. This is an arithmetic bound from the code, not a measured outage benchmark.

**Change:** give scan scheduling, enrichment/topology, alert evaluation and delivery independent supervised loops with bounded concurrency, cancellation, and lag/error reporting. A slow integration must not hold the discovery scheduler. Use the outbox worker from finding 5; no general job framework is needed.

### 7. P1 — Clock correction can leave stale measurements looking live

Evidence: `crates/hope-server/src/agents.rs:383`, `:446`, `:454`.

Reports up to five minutes in the future are accepted. Live state only advances when `collected_at` is at least the stored watermark, while `last_report` advances for accepted nonduplicate reports. Correcting a fast agent clock backward therefore freezes measurements until wall time catches up, although the host continues to look as if it is reporting normally. A larger backward correction after a previously accepted timestamp can extend the freeze.

**Reproduced:** accept a 10% CPU sample timestamped 240 seconds ahead, then accept a current 99% CPU sample. Live CPU remains **10%**.

**Change:** distinguish observed time, received time and ordering. Introduce a durable reporting epoch/sequence with restart and backlog rules, or an equally explicit clock-recovery design. Expose excessive skew and measurement freshness. Simply replacing live state on receipt would reintroduce backlog regressions.

### 8. P2 — Live events and snapshots describe different devices

Evidence: `crates/hope-server/src/agents.rs:401`, `:474`; `crates/hope-server/src/state.rs:298`; `web/components/console.tsx:307`.

Report ingestion broadcasts the raw stored `Host`; bootstrap returns the enriched projection. The browser replaces the entire device with each host event.

**Reproduced:** associate a source with an actively reporting agent host. The inventory snapshot contains its child data; the next report's emitted host event contains **no child and no source-device ID**. Polling can subsequently restore them. This can make source details and filters oscillate.

**Change:** publish the same canonical view as GET/bootstrap, or use a typed partial observation event that updates only the affected fields. Include a revision independent of report timestamps, so a live metrics update cannot mask a newer identity/settings snapshot. Cover this with a real server-to-browser event test.

### 9. P2 — A hanging browser request can permanently stop refresh

Evidence: `web/lib/api.ts:10`; `web/components/console.tsx:246`, `:318`.

The shared fetch helper has no deadline. Dashboard refresh sets `refreshPending` until that request settles; subsequent timer and SSE refresh requests return early. A connection that remains open without completing can disable snapshot recovery for the lifetime of the page. The initial authentication request can likewise leave the loading screen indefinitely.

**Evidence level:** code-traced failure path, not reproduced with a real hanging proxy in this review.

**Change:** add bounded request deadlines and cancellation, explicit retry/reconnect state, and an eventual refresh after a request ends. Give source history the same refresh/recovery policy as other history: `SourceMetricHistory` currently fetches only on host change. Extract the transport/store/hooks from the 1,451-line console before expanding more UI behavior.

### 10. P2 — A release publish can mix files across an agent installation

Evidence: `crates/hope-server/src/releases.rs:51`, `:61`; `scripts/install-agent.sh:681`, `:730`.

The server resolves `current` afresh for each request. An installation fetches the installer/key, manifest, manifest signature, binary and binary signature separately through mutable URLs. An atomic symlink swap protects one request, not the whole installation. A concurrent publish can therefore cause valid releases to fail signature/size/checksum checks. Verification correctly fails closed, so this is an availability defect, not an unsigned-code execution finding.

**Change:** use immutable release IDs or content-addressed artifact URLs and pin the complete transaction after selecting the release. Keep referenced releases available for a documented installation window. Test publishing between every download step.

## Additional work needed for a credible release

- **Discovery must recover after interface changes.** `arp.rs:56` enumerates interfaces and addresses once. A terminal reader error marks an interface inactive (`:138`); there is no reopen/re-enumeration path. Add bounded recovery and test address changes, interface replacement and raw-socket failure on Linux. This review did not reproduce those conditions on Linux.
- **Provide deliberate inventory cleanup.** There is still no host-delete API, and enrollment counts all rows in `agents` (`agents.rs:286`), including retained revoked/uninstalled identities. Device/identity turnover can exhaust the lifetime cap. Provide separate retire/forget/purge semantics with clear history and rediscovery behavior, rather than requiring SQL edits. Keep alias and association cleanup in the identity service.
- **Measure realistic collection and history costs.** The retention load fixture seeds CPU-only `Metrics` JSON (`tests/retention_load.rs:79`). It does not establish costs with disks, NICs, sensors, containers, guest filesystems, source observations or concurrent browser history reads. Scanner persistence also reconstructs the fleet while holding the global write lock. Benchmark populated collections and sustained retention catch-up before changing databases or adding caching.
- **Expose the health of monitoring itself.** Add agent queue age/count/drops, last successful collection/delivery, clock skew, background-loop lag and retention lag. `/healthz` is unconditional process liveness today; add a separate bounded readiness/degraded-status check rather than treating it as evidence that discovery, SQLite or delivery works.
- **Rerun platform gates on the actual release artifacts.** Native tests do not certify systemd, launchd, physical ARP or pmxcfs locking. The macOS service installation CI matrix explicitly remains unimplemented. Do not broaden platform claims until these are proven. The repository's prose about CI/release status also needs reconciling with current deployed workflow evidence; this review did not query GitHub run history.

## Where the complexity is justified, and where to simplify

| Area | Decision | Reason |
| --- | --- | --- |
| Rust/Axum/Tokio + SQLite/WAL + static web export | Keep | Suitable deployment model for the stated single-admin scope; changing the stack does not fix these defects. |
| Agent actor, bounded spool, credential ownership, signed updater, SSH host-key checks | Keep; refactor only around measured problems | Meaningful fault handling and acceptance tests already exist. The safety checks are useful complexity. |
| Device identity/reconciliation across `agents`, `api`, `state`, associations and Proxmox legacy paths | Rewrite orchestration incrementally | The same mutable type represents storage and view state, and several paths independently decide identity. Fix that boundary first. |
| Legacy source migrations inside normal report ingestion | Extract behind the identity service | Keep compatibility, but make reconciliation resumable and independently testable instead of extending the roughly 680-line child ingestion path. Move eligible cleanup to explicit migration/reconciliation work. |
| Server installation/upgrade | Rewrite the activation transaction | It currently lacks the recoverability already implemented for agents. Share the transaction contract between host and Proxmox packaging adapters. |
| Alerts | Small replacement of transition/delivery orchestration | Use persisted conditions plus an outbox and bounded worker. Retain the existing condition evaluators. |
| Console/details/history components | Refactor by state and behavior boundaries | `console.tsx` is 1,451 lines, `host-details.tsx` 1,941, `metric-history.tsx` 1,187. Extract transport, device store, identity actions and history hooks; file splitting alone is not the objective. |
| History/storage pipeline | Measure, then consolidate carefully | Host and source histories duplicate some behavior, but measurement semantics differ. Preserve per-key counts, unknown values and source provenance; avoid inventing a generic metric framework. |

Do not add microservices, a message broker, a plugin architecture, a new database, or more collectors as the next reliability step. A small SQLite outbox and explicit domain boundaries are sufficient for the problems observed.

## Recommended implementation sequence and release criteria

1. **Capture the failures as regressions and fix packaging.** The diagnostic fixture below records current faulty behavior; convert it into expected-correctness tests alongside the fixes. Assert the final published artifact set and checksums.
2. **Repair device identity as one coherent change.** Separate storage from views, persist manual intent, centralize reconciliation, clean up previously persisted projection fields, and make snapshots/live events consistent. Preserve existing IDs/history through migrations.
3. **Make monitoring survive integration failures.** Implement condition transitions/outbox, independent scheduling, clock recovery, browser deadlines and background-health reporting.
4. **Make server installation recoverable.** Implement versioned activation and database backup/recovery; validate fresh install, upgrade, interruption, failed activation and restore on real service managers.
5. **Run a release-candidate acceptance campaign.** Use the exact packaged artifacts for both Linux architectures and any macOS agent platform advertised. Run a multi-day soak with representative inventories, discovery, guest churn/migration, agent and server restarts, clock corrections, database/disk pressure, notification outages, and browser reconnects. Verify bounded storage/queues, measured ingestion latency and recovery without manual SQL. Set resource/latency limits from the supported deployment budget before the run.

Recommendation can begin with a narrowly evidenced Linux beta once these blockers and acceptance criteria pass. Add platform claims separately as evidence becomes available. A green unit/browser suite alone is insufficient.

## Validation performed in this review

| Check | Result |
| --- | --- |
| `cargo test --workspace --locked` | 251 passed; one retention load test intentionally ignored |
| Rust formatting and strict workspace Clippy | Passed |
| Web typecheck and ESLint | Passed |
| Production static web build | Passed |
| Playwright suite, including pure helper tests | 57 passed |
| Installer harness | 132 passed; 11 Linux-only tests skipped on macOS |
| Release assembly harness | 13 passed |
| Shell syntax and ShellCheck | Passed |
| Generated protocol drift check | Passed |
| Documentation tests | 8 passed |
| Diagnostic probes using real server/state fixtures | Four passed, confirming the faulty behaviors in findings 1, 2, 5, 7 and 8 |
| Linux/container/systemd/pmxcfs gates | Not run: Docker daemon unavailable |
| Full populated-load and long-running soak tests | Not run |

The diagnostic source is [reliability-review-2026-10-20-probes.rs](reliability-review-2026-10-20-probes.rs). It deliberately asserts the observed broken behavior, so its passing result is **evidence of defects**, not release acceptance. It reuses the baseline `tests/api.rs` helpers and is kept outside the normal suite. Its APIs are historical and do not compile against the refactored inventory boundary. To reproduce, use a separate checkout of `e564e16`, copy the diagnostic source into that checkout, and run from its root:

```sh
cp docs/audits/reliability-review-2026-10-20-probes.rs crates/hope-server/tests/reliability_review_probe.rs
cargo test --locked -p hope-server --test reliability_review_probe review_probe -- --nocapture
rm crates/hope-server/tests/reliability_review_probe.rs
```

Selected diagnostic output:

```text
notification HTTP requests after 3 evaluations and persistent 503: 1
latest CPU after accepted 99% sample = Some(10.0); watermark ahead 240s
same device: bootstrap child=true, SSE child=false, SSE source id=None
unlink then bootstrap: source still attached=true; association confirmed
projection saved then rejected: 2 displayed rows carry source id
```
