# Milestone 4 design: monitor persistence and health execution

Spec refs: §8.1–8.5, §11, §17 M4. M3 produces canonical services,
current endpoints, and reviewable monitor proposals. M4 turns an approved
proposal into a configured monitor, then records bounded check results and
incident state against that same monitor.

## 1. Approval boundary

`POST /api/v1/monitor-proposals/{id}/approve` changes a pending proposal to
`approved` and creates its monitor in the same PostgreSQL transaction. The
approval transaction therefore cannot report success while leaving a proposal
without its monitor.

The monitor retains the proposal, service, and endpoint UUIDs. Its `config`
contains the policy-generated check target and operator overrides. The server
validates interval, timeout, failure threshold, and recovery threshold before
the row is written; database checks apply the same bounds to direct writes.

Repeating approval is idempotent. If the proposal is already approved, the
server returns the existing proposal and can create a missing monitor without
resetting an existing monitor's configuration, state, or counters. Rejection
does not create a monitor, and an opposite decision remains a conflict.

Monitor creation writes one `monitors` change event in the approval
transaction. The proposal decision writes its own change and audit records.

## 2. Stored state

Migrations `0015_monitoring_core.sql`, `0016_monitoring_defaults.sql`,
`0017_monitoring_underlying_state.sql`, and
`0018_monitor_result_rollups.sql` add:

```text
monitors(id, proposal_id, service_id, endpoint_id, monitor_type, config,
         interval_seconds, timeout_ms, failure_threshold, recovery_threshold,
         enabled, state, consecutive_failures, consecutive_successes,
         underlying_state,
         last_result_at, last_success_at, last_failure_at, next_run_at,
         lease_owner, lease_expires_at, version, created_by, timestamps)

monitor_results(id, monitor_id, status, observed_at, latency_ms, error,
                details, created_at)

incidents(id, monitor_id, state, severity, opened_at, recovered_at,
          last_event_at, failure_count, last_result_id, summary, timestamps)

monitor_result_rollups(monitor_id, bucket_start, counts, latency summary,
                       last_status, timestamps)
```

The database allows at most one open incident per monitor. Results are
append-only observations until the scheduled retention job marks them rolled
up and deletes rows outside the raw-result window. Monitor state and incident
rows are projections updated by the check worker.

## 3. API slice

The authenticated inventory router exposes:

- `GET /api/v1/monitors`, with optional `state` and bounded `limit` filters;
- `GET /api/v1/monitors/{id}`; and
- the existing proposal approval route, which creates the monitor.

The list and detail responses are the persisted monitor rows. Notifications
and pagination beyond the bounded first page are not part of this slice.

## 4. Execution boundary and first worker slice

The worker's in-memory scheduler selects due enabled monitors in batches of at
most 32, leases each row in PostgreSQL, and runs checks outside the database
transaction. It applies a deterministic ±10% interval jitter when scheduling
the next run. A lost lease rolls back the result transaction so another worker
can retry the check after lease expiry.

The first execution slice supports bounded TCP connect and HTTP/HTTPS GET
checks. It uses the monitor's canonical service/endpoint association, sends no
credentials, follows no redirects, and bounds paths, headers, response body,
and assertion text. A result, health-state projection, and incident update
are committed together. One transient failure leaves the monitor below its
incident threshold; reaching the threshold opens the single permitted open
incident, and the configured recovery count closes it.

The scheduler also derives `stale` from the last result age without replacing
the persisted underlying state. A stale transition is recorded as a warning
change event and the next successful check can clear it.

The domain health state machine owns threshold counting and duplicate
transition suppression. The server worker owns leases, protocol adapters,
result persistence, incident rows, and stale detection.

DNS, ICMP, TLS-expiry, additional content/API assertion variants, notification
delivery, and result retention remain subsequent M4 slices.
