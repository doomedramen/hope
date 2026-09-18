# 0005. In-memory monitor scheduler, not one job per check

## Status
Accepted

## Date
2026-09-18

## Context
Spec §8.3 requires checks to run on configurable intervals (default 30s) with jitter, retry policy, and failure/recovery thresholds, at a scale of up to 10,000 services/checks (§5.3). Monitoring is one of three distinct scanning activities in §6.3, run far more frequently (every 30–60s) than discovery or change scans.

## Decision
Run monitor checks from an in-memory scheduler (tokio tasks plus jitter) loaded from each check's `next_due` timestamp in Postgres. The scheduler writes only results and state transitions back to the database; it does not enqueue a background job per check execution.

## Alternatives considered
- **Route every check through the general background job queue (ADR-0006)** — rejected. At thousands of checks on 30-second intervals, that queue would be dominated by high-frequency, low-value monitor executions, adding lease/heartbeat overhead to work that is inherently ephemeral and needs no retry-with-backoff semantics beyond the check's own failure threshold.
- **External scheduler (cron, Postgres `pg_cron`)** — rejected: coarser granularity than jittered sub-minute intervals, and adds an extra moving part outside the application process.

## Consequences
The job queue (ADR-0006) stays reserved for genuinely job-shaped work (scans, agent repair, updates). Monitor state must be rebuilt from `next_due` on process restart, so a brief gap in checking is possible after a restart; this is acceptable given the spec's "fail toward more information, not less" posture (§15.1).
