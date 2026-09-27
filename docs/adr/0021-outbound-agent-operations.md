# ADR-0021: outbound agent delivery, logs, and managed updates

## Status

Accepted, September 2026. Extends ADR-0017; SSH remains available for bootstrap and recovery.

## Decision

Agents collect independently of their WebSocket connection. Metrics and logs have separate durable outboxes, each limited to 64 MiB and 24 hours. The agent persists records before sending and removes them only after commit acknowledgement. Stable metric sample and log event identities make replay idempotent. Expiration, capacity eviction, corruption, and permanent rejection produce loss counters. Inventory retains its existing durable snapshot/observation pair.

Journal units and Docker containers are explicit opt-in sources. Literal redaction runs before persistence. Journal cursors and Docker timestamp/occurrence checkpoints advance only after the corresponding outbox records are durable. Unknown severity stays unknown. Source errors and collection gaps enter the agent diagnostic stream. Configurations carry revisions and acknowledgements; operator changes remain separate audit events.

Managed updates use the authenticated outbound session and the existing signed release repository. The normal service account downloads and verifies a bounded bundle. A separate root-owned systemd timer invokes a root-owned helper, which verifies the bytes again, saves the previous executable, and atomically replaces the fixed agent executable. The agent cannot supply arbitrary destinations or commands. The helper survives agent restarts and rolls back interrupted replacement or failed startup. Both forward startup and rollback require local collection to resume.

Local health does not depend on server connectivity. The server declares success only after the target version, a fresh heartbeat, metrics, and previously healthy enabled log sources resume. Quiet sources prove health through successful polling, rather than requiring a new log message. A disconnected server leaves verification pending. Root helper state, identity, outboxes, and source cursors survive replacement. Releases using this updater must retain compatibility with these persisted formats.

New managed policies default to notify-only. Automatic updates use UTC windows, deterministic agent cohorts, a 5% canary cohort, a ten-minute observation period, and three concurrent replacements. Failures or overdue unverified operations pause automatic rollout for 24 hours. A version already attempted on an agent is not automatically retried. Policy changes cancel queued automatic work. Explicit requests are idempotent and do not interrupt an active operation. Existing SSH policies are preserved independently.

Metric ingestion maintains five-minute and hourly rollups in the same transaction as raw samples. Replay cannot double-count rollups. Late samples correct extrema and averages without replacing a newer latest value. Retention removes raw samples only after rollups exist. Current UI history remains bounded to seven days; retained hourly data supports later longer-range views.

## Consequences

The root helper and its installation require a one-time bridge upgrade for existing agents. The installer supports signed Linux amd64 and arm64 bundles; publishers must provide the corresponding signed artifact. Older agents continue negotiating their existing capabilities.

Outbox bounds are protection limits, not a guarantee against full disks, unavailable filesystems, or arbitrary source volume. If disk writes fail, an in-memory loss counter and local tracing report the failure; persistence of that counter cannot be guaranteed until storage recovers. Docker logs cannot provide a durable engine cursor: rotation or exhaustion of the bounded replay boundary can require an explicit gap and restart from current time.

Log retention and a per-connection ingest budget bound ordinary workloads. A hostile authenticated agent reconnecting repeatedly is outside the per-connection budget; fleet-wide quotas and production capacity measurements remain separate work.
