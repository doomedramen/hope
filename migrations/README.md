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
These migrations do not themselves schedule checks or send notifications;
those behaviours belong to the M4 worker slices.
