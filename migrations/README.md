# Migrations

Applied automatically by `server serve` and `server migrate` via
`sqlx::migrate!`, embedded into the binary at compile time (see
`apps/server/src/main.rs`).

## M0 acceptance gate coverage (spec §17)

> Migrations apply to an empty database and upgrade from the previous
> test schema.

- **Apply to an empty database**: covered — every DB-gated integration
  test across the workspace runs `sqlx::migrate!(...).run(&pool)` against
  a throwaway `postgres:17` container with no prior state, and the
  clean-start `docker compose up` walkthrough (README quickstart) does
  the same against a fresh named volume.
- **Upgrade from the previous test schema**: covered from M1 onward by
  `apps/server/tests/migration_upgrade.rs` (DB-gated, `DATABASE_URL`
  required): applies migrations 0001-0003 (the M0 schema) as if already
  deployed, inserts sample data, then applies the full migration set on top
  and asserts the M0 data is unchanged and the new tables exist. Extend this
  test's assertions as later milestones add release-specific upgrade
  guarantees.

M3 adds `service_review_items` in migration `0012_service_review_items.sql`.
It is durable queue state; fingerprint evidence remains append-only in the
shared `evidence` table.

M3 monitor proposals land in `0013_monitor_proposals.sql`. Proposals reference
only canonical `services` and current/history-safe `endpoints` rows, use a
unique rule/endpoint identity, and preserve manual review decisions and user
overrides across refresh. This migration adds policy output only: it does not
create monitor rows, schedule checks, or execute network checks. The upgrade
test asserts that the proposal table exists after applying the full migration
set over the M0 schema.

M3 collector evidence lands in `0014_collector_evidence.sql`. It extends the
evidence source allowlist for UPnP/SSDP observations; the collector runtime
still requires an approved discovery scope and stores `LOCATION` as observed
data without fetching it.

M4 monitor persistence lands in `0015_monitoring_core.sql`. It adds approved
monitor intent, bounded check-result history, and one-open-incident-per-monitor
state. `0016_monitoring_defaults.sql` aligns new monitor rows with the M4
defaults from spec §8.3: a 30-second interval and three consecutive failures.
`0017_monitoring_underlying_state.sql` preserves the non-stale state needed to
reconstruct the health state machine after a worker restart. `0018_monitor_result_rollups.sql`
adds hourly result rollups and a marker that lets the scheduled retention job
delete raw observations only after compaction. The monitor worker and scheduled
jobs own execution, retention, and notification behaviour; migrations only
create the durable state.

M4 notification routing lands in 0019_notifications.sql. It stores provider
channels, severity/event routes, and idempotent incident deliveries. The
delivery job sends webhook or ntfy notifications outside the monitor
transaction and retries provider failures through the normal job queue.

M5 agent persistence lands in 0020_agent_inventory.sql. It extends enrolled
agent metadata, keeps one current inventory snapshot plus bounded replay
history, projects agent observations into the canonical inventory tables, and
stores one durable open/recovered heartbeat incident per agent. Offline sweeps
run through the worker queue; recovery updates incident state without deleting
the agent's current or historical inventory.

M6 credential and deployment state lands in 0024_credentials.sql and
0025_ssh_host_keys.sql. Secrets are encrypted with an externally supplied
master key, while SSH host-key trust and bounded install/repair operations
remain durable. `0023_agent_observations.sql` completes the agent observation
history used by the deployment and monitoring paths.

M7 signed update state lands in 0026_agent_updates.sql. PostgreSQL stores
verified release metadata, update policy, and durable staged-update progress;
release bytes remain in the server-side repository.

M8 dependency topology lands in 0027_dependency_graph.sql. It adds explicit
confirmation state and bounded graph safeguards for manual, deterministic,
and suggested edges. M8 reconciliation projects containment and service
ownership into deterministic inferred edges and runs through the worker job
queue.

M8 notification suppression lands in 0028_alert_suppressions.sql. It records
the root incident, dependency edge, path, target, event, and technical reason
when a downstream incident notification is suppressed. Child incidents and
monitor results are not removed.

M9 maintenance planning lands in `0029_maintenance.sql`. It stores timezone-
aware events, normalized target/required/affected/exclusive resources, and
stable occurrences expanded through the bounded future horizon. `0030` keeps
the exact maintenance occurrence behind an expected incident suppression;
`0031` stores idempotent overrun deliveries routed through configured
webhook/ntfy channels. The scheduler and reconcile job own lifecycle state,
conflict checks, and recurring expansion; no frontend calendar is required for
the backend milestone.
