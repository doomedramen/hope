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

## Work queue from evidence

- Server future-clock watermark freezes live state after a backward correction; needs robust ordering across restarts/backlog, not receipt-time replacement of every historical report.
- Container statistics are copied from the last sample rather than aggregated; container history lacks series.
- Missing uptime/boot identity, agent queue/transport/collector health, CPU breakdown/allocation, filesystem inodes/health, NIC errors/state, expected service state, container health and sustained alerts. Work through the complete reporting-gap table in the agent audit.
- Agent state writes and spool serialization costs, duplicated/expensive collection, release cross-request consistency and updater-attempt visibility.
- Operator notes in `.agent-notes/inbox.md`: host cleanup, multi-address identity, deleted-network history and raw-ARP recovery. Verify current behavior before choosing a correction.

## Environment limits

Do not claim real Linux/systemd/LXC or four-target release validation from native fixtures. Keep working on independent code while those gates remain unavailable. Hosted access currently provides read-only UI evidence; no production update or configuration change has been made by this review.
