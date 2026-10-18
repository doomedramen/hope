# Repository review: two passes

Objective: inspect all repository code twice, fixing bugs, poor code, avoidable costs, potential defects, UX/UI issues and missing monitoring features. Commit verified milestones regularly. This review is **active**, not complete.

Baseline: `21d1413`. The accompanying JSON inventories every baseline tracked file and the new reliability modules. Each file starts pending for both passes. New files must join the inventory; generated/data files need generator/consumer/contract validation and binary assets need usage/render validation. Passing a suite is not evidence that every file has been reviewed.

## Coverage plan

| Area | Pass one | Pass two |
| --- | --- | --- |
| Agent collection, scope, persistence, delivery and lifecycle | Existing targeted investigation; full review pending | Pending |
| Protocol, validation, migrations and metrics/history | Existing targeted investigation; full review pending | Pending |
| Server auth, API, storage, notifications, releases, SSH | Pending | Pending |
| Discovery, enrichment, classification, topology, uptime | Pending | Pending |
| Web state/API, all routes, views, charts and UI primitives | Pending | Pending |
| Install/update/build/release scripts and infrastructure | Existing targeted investigation; full review pending | Pending |
| Tests, fixtures, CI, generated data, documentation and tooling | Pending | Pending |

Pass one traces implementation and error cases, reproduces findings and implements missing monitoring capabilities. Pass two independently retraces the resulting code, validates cross-component contracts and renders important user flows. Both passes cover every inventoried source, not just the initial agent findings. Record the actual evidence and unresolved limits rather than marking a directory complete because selected tests passed.

## Verified milestones

- Initial agent reliability and hosted inspection: see [agent reporting audit](agent-reporting-2026-10-18.md) and [crate research](agent-crates-2026-10-18.md). Native Rust 182 passing, installer 108 distinct checks, docs 8 passing, frontend fleet 14 passing, type checking and Clippy passing. The privileged Linux/systemd gate and complete Linux target build remain unverified locally because Docker/musl C tooling is unavailable.
- `1b05bdb`: committed the initial reliability fixes and review ledger. `make check` and frontend formatting passed before the checkpoint.
- `7f6b16c`: container history. CPU/RAM gauges now use per-ID sums/counts, preserve unknown readings and retain observed containers within a bounded bucket. Both retention and display aggregation preserve each container's latest metadata across late arrivals. Migration 6 adds a host metadata timestamp. Legacy container rollups without gauge statistics expose unknowns. The details page has four history windows and retains charts when the latest report omits the container. Native Rust 186 passing (plus one separately passing updated validation regression), Clippy, frontend types/lint/format/export, protocol generation, docs 8 and browser suite 41 passing. Desktop and mobile screenshots reviewed.
- The [release-mode 250-host retention fixture](../benchmarks/retention-macos-arm64-2026-10-18.json) passed with 1,154,500 seeded samples, 0.614-second cleanup, 2.424-ms median and 8.920-ms maximum concurrent report latency. Database size was 894,586,880 bytes. This CPU-only native fixture does not establish populated-collection costs or Linux performance.
- History UI: live report caching is bounded to the 10–15-minute reload interval and 256 points; a day of five-second reports retains 181 points for the longest reload interval. Windows advance on a shared 30-second clock during outages. Errors belong to their host/window, and an unavailable GPU keeps its selector. Frontend build/types/lint passed; the expanded 43-test suite covers cache bounds, advancing windows and switching GPU adapters.
- Frontend resilience: theme choices have an in-memory fallback when local storage is denied; storage events still synchronize normal cross-tab changes. The API helper normalizes native/tuple/object headers and handles non-object proxy errors. Isolated chart observations render sample dots, including separate observations divided by outages. Frontend build, type checking, lint and 46 browser/pure tests passed; the sample-marker rendering was inspected.
- `034be2d`: corrected an anonymous chart renderer flagged by the final lint run after `7bb83b8`. Frontend lint, type checking and formatting were rerun successfully; the earlier 46-test functionality run and static export passed.
- Offline queue cost: cache each immutable report's JSON length once, retaining the existing persisted array and all count/age/byte limits. Eviction no longer serializes the backlog repeatedly or duplicates encoded reports in memory. All 48 agent unit/integration tests and workspace Clippy passed. The same native debug delivery test group took 6.69 seconds before and 0.38 seconds after (the latter also includes a new escaped-text/removal/expiry/legacy-format regression). These single local test runs are evidence of eliminated work, not a production resource benchmark.
- Update request ordering: persist the rollback cooldown durably before creating the wakeup file, and use exclusive creation for the request. Failure to save the cooldown cannot start an update. No temporary request file is placed in launchd's watched queue. Agent unit/integration tests (49) and workspace Clippy passed; real service-manager interruption remains unverified.
- Installer test portability: the full native discovery run had 125 passing cases and 11 Linux server-installer failures caused by macOS rejection. Those Linux-userland fixtures now explicitly skip on other platforms, while the independent community-wrapper contract case still runs. A diagnostic attempt to stub Linux exposed GNU `sed -i` dependencies, confirming that spoofing the platform is insufficient. The 11 server cases require Linux; no Linux result is claimed. Agent installer cases passed in the original full run.

## Work queue from evidence

- Server future-clock watermark freezes live state after a backward correction; needs robust ordering across restarts/backlog, not receipt-time replacement of every historical report.
- Container aggregation and charts are implemented; review the remaining non-container keyed-series lifecycle and history resource costs.
- Missing uptime/boot identity, agent queue/transport/collector health, CPU breakdown/allocation, filesystem inodes/health, NIC errors/state, expected service state, container health and sustained alerts. Work through the complete reporting-gap table in the agent audit.
- Agent durable state-write costs, duplicated/expensive collection, release cross-request consistency and updater-attempt visibility. Repeated spool serialization during budget checks is fixed.
- Review raw-point resolution/freshness semantics and remaining keyed-series lifecycle behavior beyond the fixed live cache and container paths.
- History/retention SQL loads full metric JSON; the existing 250-host fixture uses minimal CPU-only reports. Measure populated collections and storage growth before treating its results as representative of a full monitoring installation.
- Operator notes in `.agent-notes/inbox.md`: host cleanup, multi-address identity, deleted-network history and raw-ARP recovery. Verify current behavior before choosing a correction.

## Environment limits

Do not claim real Linux/systemd/LXC or four-target release validation from native fixtures. Keep working on independent code while those gates remain unavailable. Hosted access currently provides read-only UI evidence; no production update or configuration change has been made by this review.
