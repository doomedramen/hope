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
