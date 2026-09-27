# Agent reliability, data logging, and operator UX

Status: core implementation delivered; operational acceptance in progress, 27 September 2026.

This plan gives equal weight to reliable metrics/inventory/agent diagnostics and searchable host/container logs. Managed agent updates are also a first-release requirement. The UI must make these capabilities useful from the device being investigated. The core delivery, logging, managed-update, and investigation flows are implemented. See the implementation evidence below; the later capacity and usability gates remain open.

## Outcome

An operator can install and update an agent, verify that its data is arriving, find a resource spike, inspect the corresponding logs, and understand any missing data without leaving the device context. A server outage or agent upgrade does not silently discard data within the configured buffering budget.

Keep the existing Rust agent, authenticated WebSocket transport, PostgreSQL storage, React UI, and signed update machinery. Extend them in small vertical slices. Do not add a separate logging service before measuring the existing stack against a representative workload.

## Findings from the current code

These findings come from source inspection, not a live usability session or a production load test.

| Area | Existing foundation | Gap to address |
| --- | --- | --- |
| Delivery | Inventory and observations persist together for replay; metrics have sample IDs and transactional deduplication. | `run.rs` keeps one metric batch in session memory. Reconnect discards it. A pending batch prevents collecting another sample. Collection only runs inside the connected session. |
| Runtime | Reconnect jitter, bounded collectors, and capability negotiation exist. | Collection is awaited inside the transport loop. Slow probes delay message handling. Heartbeat acknowledgements have no explicit client watchdog. Rejected batches lack a clear retry/terminal outcome path. |
| Diagnostics | Server JSON tracing, local agent tracing, collector status, and heartbeat incidents exist. | There is no first-class searchable agent diagnostic stream. Snapshot-derived observations retain status but omit the collector error reason. |
| Host/container logs | Docker inventory and systemd inventory exist. | No journal/container log protocol, ingestion store, query API, or viewer was found in the inspected paths. |
| Metrics | Samples every 15 seconds, seven-day default retention, freshness, dimensions, charts, and bounded output aggregation exist. | History requests fetch all matching raw rows before aggregation. Buckets group by row count. Future timestamps can affect freshness; query lacks an upper time bound. UI discards missing points and displays only each bucket's latest value. |
| Agent fleet | Cursor-based list API, device associations, detail/history APIs, update APIs, and enrollment command exist. | Agents UI fetches one page, searches/counts that page, auto-selects the first agent, and keeps selection/tab/range outside the URL. |
| Agent updates | Signed releases, manual/notify/automatic policies, rollout percentages, durable operations, and SSH-based replacement/rollback exist. | Updates require SSH credentials and host access. Agents installed through the enrollment command need a managed update path using their existing outbound connection; update controls and outcomes need UI exposure. |
| Agent UI | Inventory tabs, resource charts, accessible selection buttons, and explicit empty/error states exist. | Eight detail tabs compete inside a narrow pane. No unified activity/log investigation or enrollment-to-first-data completion view. |
| Capacity | M10 documents a boundary of 500 agents. | Existing scale harness measures job/monitor claims, not telemetry ingestion, history queries, logs, or browser performance. |

Primary evidence:

- [Agent runtime](../../apps/agent/src/run.rs), [collectors](../../apps/agent/src/collectors.rs), and [protocol](../../crates/protocol/src/lib.rs).
- [Metric ingestion and queries](../../apps/server/src/agent_metrics.rs), [inventory and fleet API](../../apps/server/src/agent_inventory.rs), and [retention jobs](../../apps/server/src/inventory/retention.rs).
- [Agents UI](../../apps/web/src/components/AgentsPage.tsx), [existing UI tests](../../apps/web/src/components/AgentsPage.test.tsx), and [capacity scope](../operations/performance.md).
- [Existing update orchestration](../../apps/server/src/agent_updates.rs) and [signed-update architecture](../adr/0017-signed-agent-updates.md).

## Delivery sequence

The first release includes durable telemetry, managed agent updates, and a usable journal/container log viewer. Later phases deepen history, fleet controls, and scale. Effort bands below mean small (roughly 1–3 engineering days), medium (4–7), or large (8–15), including focused tests. They are planning estimates, not delivery commitments.

### 1. Make collection and delivery independent — priority P0, large

Split the agent into collection scheduling, a durable local outbox, and transport. Keep heartbeat handling responsive while collectors run. Use bounded task concurrency and per-collector deadlines; put blocking host reads outside the transport loop.

Persist metrics, observations, diagnostic events, and later log batches with stable identities before sending. Reuse the inventory replay behavior while defining separate policies: inventory can coalesce superseded snapshots; metrics and logs retain samples until acknowledged or explicitly evicted. Record each eviction with stream, time interval, count, and reason.

Define configurable byte and age budgets, bounded batch sizes, retry backoff, acknowledgement deadlines, and fair scheduling. A log burst must not starve heartbeats, inventory, or metrics. On disk-full or corrupt spool records, preserve valid records, report degraded delivery, and continue liveness reporting. Sampling must continue during server disconnection.

Add typed protocol errors that distinguish temporary failures from invalid data, unsupported capability, and revoked identity. Quarantine permanently rejected records so they cannot block every later sample. Retry transient failures without creating duplicate stored data. Define process uptime separately from connection age and reset reconnect backoff after a healthy session.

Acceptance:

- Disconnect for 30 minutes, restart the agent during the outage, then reconnect: every retained sample arrives once in the read model.
- Drop acknowledgements and replay batches: no duplicate metrics or events; source cursors do not advance prematurely.
- Delay a collector, flood logs, fill the spool, and inject an invalid record: heartbeats continue and data loss is explicitly accounted for.
- Old agents still connect; new streams require negotiated capabilities. Server changes ship before agents use them.

Primary changes: `apps/agent/src/run.rs`, extracted runtime/outbox modules, collectors, protocol, gateway, and ingestion tests.

### 2. Make agents update themselves reliably — priority P0, large

Build on the existing signed release repository, policies, operation records, and rollback machinery. Add a negotiated update capability so enrolled agents can receive a typed update request over their existing authenticated outbound connection and download the selected signed artifact over HTTPS. Routine updates must not require inbound SSH or re-enrollment. Keep SSH as a bootstrap and recovery path; record the architectural change from ADR-0017 in a follow-up ADR.

Expose **Update now**, **Notify only**, and **Automatic updates**, with channel, version pin, allowed update window, and rollout controls. Preserve existing policies; new installations default to notify-only until the operator enables automatic updates. Separate installing a new server version from permission to restart agents. Offline agents show a pending update and revalidate policy, compatibility, and release availability when they reconnect.

The local updater verifies manifest and artifact signatures, digest, size, platform, architecture, and protocol compatibility before replacing anything. Stage the new binary, persist the operation and previous version, flush the outbox, and switch atomically. A supervised updater or helper must survive the agent exiting and restore the previous binary if the new process cannot start or fails its health deadline. Preserve identity, configuration, spool records, and log cursors. Make state migrations rollback-compatible or reject the upgrade before replacement.

Distinguish local startup health from server verification. Mark success only after the server observes the target version, fresh heartbeats, and resumed data from previously working enabled streams. Do not let an already-unavailable optional collector block every update. A network outage leaves verification pending; do not claim success or successful rollback without evidence. Record any unavoidable sampling gap during restart, then replay buffered data and resume logs from their cursors.

Roll out to a small deterministic canary group first, then expand with a concurrency limit and observation period. Pause expansion when update failures or new telemetry failures exceed configured thresholds. A rolled-back release must not be automatically retried indefinitely. Offer an explicit retry after the cause is resolved. Cancel pending work when policy changes; do not interrupt an atomic replacement halfway through.

Show current and target version, release notes, update availability, policy, eligibility/blocking reason, queued/download/verification/restart/health-check progress, and rollback outcome. Put a labeled update action in agent Overview, policy controls in Settings, and operation history in Activity. Fleet selection supports bulk updates with an individual result for every agent. Link failures directly to update diagnostics and relevant logs.

Existing agents cannot gain this capability through a message they do not understand. Ship a signed bridge release and a documented one-time upgrade using the current SSH updater or an in-place manual installer. Preserve identity and state during that bootstrap. Show **Bootstrap required** for older agents instead of implying they can already self-update.

Acceptance:

- Update a supported enrolled agent using only outbound connectivity, then verify its target version and resumed telemetry/logs without re-enrollment.
- Bootstrap an existing agent, preserve its identity/history, then perform its next update without SSH.
- Reject tampered artifacts, incompatible platforms/protocols, and unsafe state migrations before replacing the running binary.
- Interrupt download, replacement, restart, and server verification; recover deterministically with durable progress and no silent loss of buffered records.
- Exercise failed startup and rollback; distinguish restored service from a rollback attempt that also fails.
- Verify automatic policy, pins, update windows, offline deferral, canary pause, duplicate requests, and bounded concurrent updates.

Primary changes: agent updater/helper, negotiated update protocol, authenticated artifact delivery, existing update orchestration/policy migrations, installer/bootstrap path, and fleet/detail UI.

### 3. Ship searchable logs with useful agent diagnostics — priority P0, large

Add two opt-in sources first: systemd journal units and Docker container stdout/stderr. Start agent diagnostics with connection changes, collector errors/recovery, rejected batches, spool pressure, and update outcomes. Offer a small bounded local diagnostic buffer so connection failures remain inspectable before transport recovers.

Use a shared event envelope: schema version, event ID, agent ID, boot/session ID, source, source cursor or sequence, observed time, received time, severity, message, and bounded attributes. Attach device/service/container IDs only where association is known. Preserve raw source names when association is uncertain. Never use timestamps alone for deduplication.

Journal ingestion resumes from journal cursors. Docker ingestion defines a replay boundary and stable event identity that distinguishes repeated identical lines; timestamps alone are insufficient. Advance durable checkpoints only after the corresponding data is durable locally, and release outbox records only after server commit acknowledgement. Detect rotation, expired cursors, restarts, unsupported Docker log drivers, multiline truncation, and collection gaps.

Make source selection explicit: chosen units/containers, minimum severity where available, rate/byte budget, and retention. Preserve unknown severity instead of guessing that every stderr message is an error. Collect neither environment variables nor arbitrary files by default. Redact configured secret patterns before persistence and transport; render log text as text, never HTML. Apply existing operator authentication to search and exports.

Add bounded log storage and cursor pagination, with filters for time, device/agent, source, unit/container, severity, and text. Start with PostgreSQL and indexes matching these filters. Measure before choosing a larger search engine. Return a stable ordering and an explicit truncation/limit indicator. Keep operator audit events distinct from agent diagnostics and host logs.

Acceptance:

- Emit marked journal and container entries; find them through the UI, including repeated identical entries and multiline messages.
- Restart agent/server and rotate sources: either resume without silent loss/duplicates or display a precise gap event.
- A noisy container cannot exhaust the whole fleet's budget. Search, export, retention, and redaction honor configured bounds.
- Permission denied and unsupported sources show a reason and remedy without marking unrelated collectors failed.

Primary changes: new agent log collectors, shared protocol types, gateway ingestion, migrations, query endpoints, and log-view components.

### 4. Deliver the first coherent investigation flow — priority P0, medium–large

Continue the device-focused direction in [simple-ux.md](simple-ux.md) and the current top navigation. Keep Agents available for fleet administration. Use the same data projections in device details and agent details.

The default fleet view is a full-width list. Show device/host name, last contact, collection state, delivery backlog, and version/compliance. Replace four large summary cards with compact filter counts. Search, filters, totals, and pagination operate across the entire fleet through the API. Keep row order stable during refresh.

Selecting an agent opens a substantial detail view with four sections:

- **Overview:** connection state, last successful collection per stream, delivery backlog, current exception, associated device, and next useful action.
- **Metrics:** CPU, memory, disk, network, pressure, and GPU only when available. Keep interface/device selection and time range visible.
- **Logs & activity:** searchable host/container logs and a clearly labeled agent-event mode. Preserve source identity; distinguish operator actions and diagnostic events.
- **Inventory & settings:** host inventory grouped by subject, collector configuration, log sources, retention, and update controls.

Use URLs for agent/device selection, section, filters, and time range. On mobile, open a full detail page with a back link that restores list state. Preserve keyboard focus and scroll position. Keep stale cached data visible during refresh failures, with a local error and last successful refresh time.

Connect charts to logs: selecting a time interval opens logs for the same device and interval. Use actual timestamps on the x-axis, show missing intervals as gaps, and label aggregation resolution. Offer average and min/max context so a resource spike is not hidden by the last value. Distinguish zero, unsupported, permission denied, stale, and not yet collected.

Log viewer behavior: pause/resume live updates, keep scroll position while reading older entries, show a new-entry count, expand structured attributes, copy a deep link, and export a bounded filtered range. Do not shift rows under the user's selection.

Enrollment finishes with real milestones: command created, agent authenticated, first inventory accepted, first metrics accepted, and selected log sources receiving data. Correlate completion to the enrollment attempt; do not infer success from another agent connecting. Logs remain opt-in and do not block basic enrollment success.

Acceptance:

- From a device, locate a resource spike and related log entries without reselecting the device or time range.
- Find an agent beyond the first 100 results; counts and search remain accurate.
- Deep links and browser Back restore context. All primary actions work using a keyboard and at 375px width.
- Five representative operators complete install verification, stale-data diagnosis, and log investigation; record time, wrong destinations, and misunderstood states.

Primary changes: split `AgentsPage.tsx` into focused components, add URL state, extend fleet summaries, and share device investigation components.

### 5. Make history trustworthy and bounded — priority P1, medium–large

Define a metric catalog with units, gauge/counter semantics, valid ranges, dimension limits, and explicit unavailable reasons. Keep collected and received times. Mark clock skew and late arrivals; reject or quarantine implausible timestamps according to a documented tolerance. Freshness must not treat an arbitrarily future sample as permanently fresh.

Replace row-count aggregation with fixed time buckets and database-side aggregation or durable rollups. Bound both database work and response size, including the one-hour query. Preserve sample counts, min/max/average/last values, missing intervals, and late-arrival corrections. Retain raw data until required rollups are durable.

Proposed starting policies: seven days of raw metrics (existing default), 30 days of five-minute rollups, 180 days of hourly rollups, seven days of host/container logs, and 30 days of agent diagnostics. These defaults require a measured storage budget; expose effective settings and estimated usage. Do not change existing audit retention implicitly.

Add ingestion observability: accepted/replayed/rejected counts, lag, per-stream last success, queued bytes, oldest queued record, dropped records, collector duration, query latency, and retention progress. Avoid creating an event for every successful heartbeat.

Acceptance:

- Future/out-of-order samples, clock corrections, replays, and counter resets produce correct histories and freshness.
- Charts preserve a short spike and a collection gap across every supported range.
- Retention preserves current inventory and required rollups; reports explain what history remains available.

### 6. Extend collection controls and prove capacity — priority P1, medium–large

Build on the update controls delivered in phase 2. Confirm API coverage before adding revoke or collection controls. Audit operator changes and make bulk results explicit per agent.

Introduce versioned collector/source configuration with acknowledged effective values. Show permission requirements and runtime availability per collector. Preserve manual install and unassociated-agent workflows. Separate enrollment expiry, trust/authentication failures, unsupported versions, and collector permission problems in diagnostics.

Extend performance tests beyond the current scheduler harness. At the documented 500-agent boundary, 15-second sampling means 2.88 million metric rows/day before rollups. Measure payload sizes, indexes, WAL, retention load, and reconnect catch-up. Define a separate log workload in entries/second and bytes/second, including sustained traffic and bursts; agent count alone cannot size logs.

Proposed gates on recorded reference hardware: p95 accepted-data visibility within 30 seconds under normal load; p95 fleet/history/log first-page API latency below one second under the agreed mixed workload; bounded memory and disk during reconnect storms. Establish CPU/RSS baselines for the agent and fail measured regressions against agreed budgets. These are targets to validate, not current guarantees.

Acceptance includes real Linux journal and Docker tests, database migrations, authentication/revocation, offline recovery, mixed-version agents, slow consumers, source flooding, mobile/keyboard flows, and a clean restore of retained history.

## Execution and backlog hygiene

Start with four reviewable contracts: delivery/acknowledgement behavior, update/rollback behavior, log record/source behavior, and shared freshness/status vocabulary. Then deliver one vertical slice: one Linux agent collecting metrics plus selected journal/container logs, surviving disconnection and an upgrade, and exposing a device-linked investigation screen. Design updater state compatibility alongside the outbox before either storage format is finalized. Expand fleet controls only after this slice works end to end.

Treat telemetry and log ingestion as equal release requirements, with managed updates also required. Their shared outbox is a prerequisite, not a reason to postpone host/container logs indefinitely. UI prototyping can proceed against representative fixtures while these contracts settle.

Revalidate existing tickets before creating duplicates. Tickets 046 and 047 describe empty-state and keyboard-selection problems already addressed in the inspected source. Ticket 052's route-focus handling also exists. Ticket 053's sidebar proposal conflicts with the newer device-focused design and current top navigation. Verify behavior, then explicitly close or supersede stale tickets through the project's ticket workflow; this plan does not change ticket status.

Track completion by demonstrated operator flows and failure recovery, not by adding more charts or increasing collected data. First release is complete only when agents can update and recover reliably, telemetry and selected host/container logs survive the agreed outage budget, and operators can investigate all three together.


## Implementation evidence and remaining gates

Implemented: independent collection with durable metric/log outboxes; idempotent log ingestion; opt-in journal/Docker sources with redaction and cursor persistence; versioned settings; signed outbound updates with a separate supervised helper; local rollback health and server stream verification; update policies and controls; server-wide fleet search/counts/pagination; URL-based detail views; log search, pause, export, and chart links; transactional fixed-time rollups and retention.

Automated checks include real PostgreSQL migrations and 300 passing Rust tests (two pre-existing opt-in checks remain ignored), plus 82 UI tests. Focused tests exercise replay, revocation, optimistic settings, late rollup samples, required update streams, and stable paused log rows. Native Linux installer and journal redaction have also been exercised on a disposable Docker target. Additional update and recovery evidence is recorded in the operational validation report when complete.

Still open: five-operator usability sessions; production-size mixed workload measurements and storage sizing; a real 30-minute outage; enrollment-attempt milestone correlation; user-configurable per-source rate/severity budgets; storage-usage estimates; a complete metric catalog; and longer-than-seven-day UI ranges. The current per-connection log budget is not a fleet-wide quota. These remain explicit gates or follow-up work rather than claimed guarantees.

Operational instructions: [Agent data and updates](../operations/agent-data-and-updates.md). Architecture: [ADR-0021](../adr/0021-outbound-agent-operations.md).


The first-view hierarchy is now a project rule in `AGENTS.md`. Agent details show connection/collection health, current CPU and memory, metric freshness, log source status, update state, and an appropriate action before technical metadata. The 375 × 812 viewport check placed these essentials within the first 570 pixels, with no horizontal overflow. Fleet rows show resource and delivery summaries; mobile uses compact agent rows instead of requiring horizontal table scrolling. Bulk updates report a separate result for every selected agent.
