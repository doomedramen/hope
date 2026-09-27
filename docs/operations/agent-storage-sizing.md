# Agent storage sizing — 27 September 2026

The measured workload needs about **346 GiB of planning capacity for 500 agents with default retention and one host log per second per agent**. The current machine had approximately 44 GiB free before calibration. Throughput passed; full-retention storage does not fit this machine at that rate.

This is a planning estimate based on measured compact PostgreSQL tables and indexes. It is not a full-retention deployment certification. No existing agent, collection rule or retention setting was changed.

## Calibration

The completed ten-minute TLS soak database was copied to the disposable `hope_storage_sizing` database. Only the copy was compacted with `VACUUM FULL`. Compaction establishes a reproducible baseline; it is not the production retention strategy. Production retention uses bounded deletes and normal PostgreSQL maintenance.

| Data | Measured rows | Table, indexes and TOAST per row |
| --- | ---: | ---: |
| Raw metrics | 20,000 | 1,136 bytes |
| Host/container logs | 570,000 | 784 bytes |
| Mature rollups with repeated fixture readings | 20,000 | 1,416 bytes |
| Mature rollups with varied readings | 20,000 | 2,920 bytes |

The one-metric historical seed was excluded from rollup calibration. Mature buckets came from the full synthetic resource payload. A second fixture varied averages, extrema and timestamps while preserving counts and sum/average consistency. Its weaker compression more than doubled the rollup allocation. The planner uses that larger measurement. This remains a sensitivity fixture, not a worst-case bound for arbitrary metric dimensions or payloads. Compression of raw metrics and logs also depends on the workload.

Evidence: [physical calibration](validation/agent-storage-calibration-2026-09-27.json), [scenario calculations](validation/agent-storage-plans-2026-09-27.json), and [original workload/environment](agent-capacity-validation.md).

## Model and scenarios

Fixed policies are metrics every 15 seconds, five-minute rollups for 30 days, hourly rollups for 180 days, and diagnostics for 30 days. The calculations assume 24 diagnostics per agent per day at the measured log-row cost. Actual diagnostic frequency and width must be measured for a deployment.

At 500 agents, the full windows contain 20.16 million raw metric samples, 4.32 million five-minute buckets and 2.16 million hourly buckets. One host log per second produces another 302.4 million retained logs over seven days.

The planning allowance adds 30% to retained tables/indexes for maintenance and growth, plus 8 GiB for active WAL. These allowances are explicit assumptions, not measured worst-case guarantees. WAL archiving, replication backlog, backups, inventory, audit history and other application data need separate budgets. The original soak's WAL generation rate must not be treated as its permanently retained WAL size.

For this machine, the comparison uses a **36 GiB agent-data budget**, leaving roughly 8 GiB of the previously free host space outside the agent budget. This is a proposed allocation, not an enforced quota. The disposable calibration copy also consumes disk until explicitly removed.

| Scenario | Agents | Raw/host log days | Host logs per agent | Planned GiB | Fits 36 GiB model? |
| --- | ---: | --- | --- | ---: | --- |
| Default retention, normal tested rate | 500 | 7 / 7 | 1 per second | 346.13 | No |
| Default retention, recurring test bursts | 500 | 7 / 7 | 1.9 per second on average | 604.53 | No |
| Short retention, naturally quiet selected sources | 500 | 1 / 1 | 1 per minute | 35.93 | Barely; requires validation |
| Smaller fleet, normal tested rate | 40 | 7 / 7 | 1 per second | 35.05 | Within model only |

Reducing raw retention does not reduce the fixed rollup windows. The varied rollups alone account for about 17.65 GiB before headroom. Quiet-source assumptions mean the **observed rate after intentional source/severity selection**, not permission to discard an active source's logs to force it into a budget. A quota can drop records and is not a substitute for adequate storage.

The 500-agent quiet scenario leaves very little room beyond its stated allowances. Do not certify it from this calculation alone. Verify actual payloads, retained cardinality, table/index growth, autovacuum and retention catch-up over representative windows. The default-rate 500-agent scenario requires a substantially larger storage allocation or an explicitly accepted change in data/retention requirements.

## Reproduce and adjust the plan

The calculator is read-only and uses only Python's standard library:

```sh
python3 scripts/agent-storage-plan.py
python3 scripts/agent-storage-plan.py --agents 500 --budget-gib 400
python3 scripts/agent-storage-plan.py --agents 500 --raw-days 1 --log-days 1 \
  --logs-per-second 0.016666666666666666 --budget-gib 36
PYTHONDONTWRITEBYTECODE=1 python3 scripts/test-agent-storage-plan.py
```

Its JSON includes every input, per-stream rows and GiB, the planning-budget comparison, maximum agents at the same rates/policies, and model limits. `--calibration` accepts a fresh measurement in the same schema. A successful CLI exit means the calculation succeeded; use `fits_planning_budget` to inspect the comparison. A “fits” result is not operational certification.

To repeat physical calibration, first finish the opt-in TLS soak, then create a **new disposable copy** with this exact name. Both commands refuse an existing fixture rather than overwriting it. Allow additional free space for the copy, table rewrites and WAL. Replace the container and source database below with the actual disposable soak fixture:

```sh
docker exec YOUR_POSTGRES_CONTAINER createdb -U USER \
  -T COMPLETED_SOAK_DATABASE hope_storage_sizing
docker exec -i YOUR_POSTGRES_CONTAINER psql -q -At -U USER \
  -d hope_storage_sizing < scripts/agent-storage-calibration.sql > /tmp/calibration.json
python3 scripts/agent-storage-plan.py --calibration /tmp/calibration.json
```

The SQL rejects other database names and refuses pre-existing calibration tables. The copy is preserved for inspection. It is never automatically deleted. Keep full-retention deployment acceptance open until the intended storage allocation and workload have been validated; the initial sizing analysis is complete.
