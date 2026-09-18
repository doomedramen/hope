# 0006. Custom PostgreSQL-backed job queue

## Status
Accepted

## Date
2026-09-18

## Context
Spec §5.1 calls for "a PostgreSQL-backed job queue with leases, retries, and idempotency keys" instead of mandatory Redis/Kafka. §14.1 requires idempotency keys for mutating background actions (scans, agent repair) and job IDs for long-running work. Background jobs here are scans, fingerprinting runs, agent installs/repairs, and updates — not monitor checks (ADR-0005), which bypass the queue entirely.

## Decision
Implement a purpose-built job queue as a Postgres table, using `FOR UPDATE SKIP LOCKED` for worker dequeue, a lease + heartbeat for crash recovery, exponential backoff for retries, a unique idempotency key per logical job, a JSON progress column, and a cancel flag workers poll.

## Alternatives considered
- **`apalis`** (Rust job-queue crate) — rejected. Its generic job model doesn't map cleanly onto the exact lease/idempotency/progress/cancel semantics spec'd in §5.1 and §14.1, and adopting it would mean either fighting its abstractions to get exact control or exposing internals as workarounds. A queue this central to reliability behaviour is cheap enough to own directly and avoids depending on an external crate's API evolving out from under core scan/repair workflows.
- **`pgmq`** — rejected for the same reason: a solid generic Postgres queue, but it doesn't natively express per-job lease+heartbeat-with-progress and idempotency-key-based dedup as first-class columns, and wrapping it adds a layer without removing the need to build the same schema anyway.
- **Redis-backed queue (e.g. `sidekiq`-style)** — rejected per §5.1: avoid a mandatory Redis dependency for a homelab-scale deployment.

## Consequences
Full control over job semantics and no external queue dependency to operate or back up, at the cost of owning the queue's correctness (crash-safe leasing, backoff tuning) ourselves. Job state is visible directly via SQL, which simplifies debugging and the `/api/v1/jobs` resource (§14.1).
