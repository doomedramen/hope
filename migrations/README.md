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
- **Upgrade from the previous test schema**: **not meaningfully testable
  yet**. This milestone (M0) is the project's first, so there is no
  earlier released schema to upgrade *from* — `0001_init.sql`,
  `0002_agents.sql`, and `0003_sessions.sql` have all only ever been
  applied together, never independently released and then upgraded.
  Once M1 ships a new migration on top of an M0 database that's already
  been deployed, add a test/CI step that applies migrations through the
  last released version, then applies the new one(s) on top, and asserts
  it succeeds without data loss.
