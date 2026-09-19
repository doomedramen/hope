# 0014. Explicit low-impact discovery policy and pacing

## Status

Accepted

## Date

2026-09-19

## Context

`discovery_scopes.scan_profile` already accepts `normal` and `low_impact`, but
the worker previously ignored the value. A low-impact scan must reduce load on
fragile homelab devices without sampling away high-numbered ports: M2 still
requires a complete TCP scan from port `1` through `65535`. Scan jobs also need
to remain cancellable and keep their PostgreSQL leases alive while pacing.

The stored TCP and per-host values are operator limits. A profile resolver must
not silently turn a low-impact request into a higher-pressure scan. Repeated
workers or recurring runs should not all begin with one synchronized burst.

## Decision

Resolve the persisted profile when loading a confirmed scan run.

- `normal` preserves stored global, scope, and per-host TCP concurrency.
- `low_impact` caps global and scope concurrency at 8 and per-host concurrency
  at 1. It never raises a stored value.
- Classification fan-out is bounded by both stored TCP limits and stored
  per-host limits. Its normal cap is 8; its low-impact cap is 2.
- Low-impact TCP starts use a 10 ms shared per-run interval, deterministic
  jitter up to 10 ms, and a batch backoff of 0/5/10/20 ms with a 20 ms cap.
  Classification uses separate 20 ms, 10 ms, and 0/5/10/20 ms bounds.
- The complete TCP target/port plan stays unchanged. Pacing runs before probe
  launch and outside scanner semaphores, so every planned port still runs
  unless cancellation, lease loss, or a real scanner failure stops the run.

Pacing uses injected `Clock`, `JitterSource`, and `Sleeper` seams. Production
uses Tokio time and a deterministic SplitMix-derived source seeded by the run
UUID and probe coordinates. The bounded schedule spreads concurrent runs while
making one run reproducible in tests and after a retry.

Pacing waits race cancellation. The existing cancellation monitor and lease
heartbeat remain active during waits; lease loss never writes a successful
completion or closure evidence.

M2 does not add a maintenance or quiet-period subsystem. Low-impact policy is
not a maintenance bypass. A later milestone must add maintenance eligibility
at the transactional scheduler/enqueue boundary.

## Alternatives considered

- **Skip ports or probe a reduced port set** — rejected because Docker and
  custom services can use any TCP port, and partial scans cannot support
  authoritative closure evidence.
- **Raise or replace stored limits for low-impact mode** — rejected because
  operator limits are upper bounds and must remain effective safety controls.
- **Use wall-clock random sleeps per probe** — rejected because unbounded,
  non-reproducible delays complicate tests, retries, and lease reasoning.
- **Add maintenance checks to the worker now** — rejected because M2 has no
  persisted maintenance model; the correct future gate is scheduler/enqueue
  policy, not a worker-side bypass.

## Consequences

Low-impact scans take longer and classify fewer open ports concurrently, which
reduces pressure on devices and application protocols. They still eventually
visit every planned TCP port when not cancelled or failed. Normal scans keep
operator-configured TCP concurrency and the existing classifier bound, subject
to the same upper-bound rule.
