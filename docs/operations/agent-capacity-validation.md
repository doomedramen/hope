# Agent capacity and restore measurements — 27 September 2026

These are local acceptance measurements, not a sustained production capacity guarantee. Reference host: Apple M4 Pro, 12 CPU cores, 24 GiB RAM. Docker Desktop exposed 12 CPUs and about 7.75 GiB RAM to PostgreSQL 17. Both workloads used separate empty databases; no production data was loaded or changed.

## Mixed ingestion and query workload

The opt-in Rust harness creates 500 identities and calls production metric/log ingestion and API handlers against PostgreSQL. Four normal rounds represent metrics every 15 seconds and logs at one entry per second per agent, delivered in batches of 15. A catch-up round adds 30 metric samples and 30 logs per agent, then replays both batches to check deduplication. Sixteen concurrent ingestion tasks share a 24-connection database pool; fleet, 180-day history and full-text log queries run during ingestion.

- Exactly 17,000 metric rows and 45,000 log rows remained after replay.
- Total elapsed: 63.72 seconds.
- Mixed ingestion p95, including queueing and replay: 10.01 seconds.
- API handler p95: fleet 21.83 ms, history 10.74 ms, log search 4.65 ms.
- Database allocation grew 103,473,152 bytes; WAL grew 151,170,840 bytes.
- Final relation sizes including indexes: metric samples 22,396,928 bytes, logs 32,514,048 bytes, rollups 48,668,672 bytes.

The rollup size includes repeated-update allocation and possible bloat. Do not extrapolate it as bytes per future logical bucket. These measurements do not include months of retained data, retention jobs under concurrent load, checkpoint cycles, sustained log floods, or production network/TLS cost. Payload estimates shown in Settings are a separate, explicitly limited measurement.

## Authenticated WebSocket workload

A second harness opens 500 simultaneous authenticated WebSocket connections through the real challenge/signature and gateway handlers. All clients negotiate capabilities, then send four 15-second rounds of a metric sample, 15 log events and a heartbeat. It checks all three acknowledgements and database row counts.

- Exactly 2,000 metric rows and 30,000 log rows arrived.
- Combined metric/log/heartbeat acknowledgement p95: 1.464 seconds.
- Transport used loopback WebSockets. TLS termination, real network latency, browser rendering and long-running connections are excluded.

## Restore drill

A custom-format `pg_dump` of the mixed-workload database restored into a separate empty PostgreSQL database with `pg_restore --exit-on-error`. Sorted full-row SHA-256 hashes matched for all 500 agent records, 17,000 metric samples, 45,000 log entries and 2,000 rollup rows. This verifies retained data round-trip; it does not establish a production recovery-time objective or secret/key disaster recovery.

Machine-readable evidence: [mixed workload](validation/agent-capacity-2026-09-27.json), [transport](validation/agent-transport-2026-09-27.json), [restore hashes](validation/agent-restore-2026-09-27.json).

## Reproduction

Each test requires its own empty disposable PostgreSQL database and refuses to run if agent records already exist. It leaves data for inspection and restore testing.

```sh
HOPE_CAPACITY_DATABASE_URL=postgres://USER:PASSWORD@HOST/DISPOSABLE_DB \
  HOPE_CAPACITY_REPORT=/tmp/hope-capacity-result.json \
  cargo test -p server mixed_500_agent_workload -- --ignored --nocapture

HOPE_TRANSPORT_DATABASE_URL=postgres://USER:PASSWORD@HOST/OTHER_DISPOSABLE_DB \
  HOPE_TRANSPORT_REPORT=/tmp/hope-transport-result.json \
  cargo test -p server authenticated_500_websocket_workload -- --ignored --nocapture
```

## Sustained local TLS workload

The agreed reference target is 500 agents on the current Docker development environment. The opt-in `sustained_500_agent_tls_workload` harness exercises the production challenge/signature, WebSocket and ingestion handlers through certificate-verified TLS. The server and simulated clients share one native Rust test process; PostgreSQL runs in Docker Desktop. This matches the existing local preview arrangement, not an all-container deployment.

The default run holds 500 connections for 40 rounds at 15-second intervals (10 minutes). Each agent sends a synthetic 1,476-byte resource payload covering CPU, memory, four interfaces, two disks, pressure and delivery state. Logs average one event per second between bursts; every tenth round delivers 150 events per agent, split into protocol-bounded batches. At rounds 10, 20 and 30 all clients disconnect before reading acknowledgements, authenticate again and replay identical record identities. A rotating tenth of clients delays reading acknowledgements by two seconds. Final counts must match every generated live metric and log exactly, despite replay.

Fleet, 180-day history and full-text log handler queries run concurrently. The fixture includes 15,000 older hourly buckets spread over 30 days. Each minute adds 1,000 expired raw samples and 1,000 expired logs, runs production rollup backfill and retention, and forces a PostgreSQL checkpoint. The fixture is intentionally bounded; it does not represent all rows produced by the full default retention windows. The query measurements include database/pool/handler work, but exclude HTTP session middleware and browser rendering.

Regression thresholds are normal/burst acknowledgement p95 below five seconds, each query p95 below one second, and maximum reconnect/replay completion below one 15-second collection interval. Any missing record, duplicate identity, rejected frame, acknowledgement timeout, retention mismatch or threshold failure fails the test. Raw latency distributions and storage/WAL growth are saved even if a final latency threshold fails.

The 27 September run passed all thresholds in 600.64 seconds:

- Exactly 20,000 live metric samples and 570,000 live log entries remained after 1,500 lost-acknowledgement reconnects.
- Normal/burst acknowledgement p95 was 943 ms; maximum was 4.397 seconds.
- Reconnect/replay p95 was 4.840 seconds; maximum was 4.903 seconds.
- Query p95: fleet 400 ms, history 86 ms, log search 20 ms across 548 samples each. Individual history requests reached 2.663 seconds during load; the p95 target is not a maximum-latency guarantee.
- Ten backfill/retention/checkpoint cycles removed all 10,000 expired metrics and 10,000 expired logs. The slowest cycle took 5.973 seconds.
- Database allocation grew 541,409,280 bytes; PostgreSQL WAL position advanced 1,799,151,296 bytes. WAL is cluster-wide, so the small ongoing preview workload is included.
- The combined native server/client process peaked at 600,424,448 bytes (about 573 MiB) and averaged 24.36% of one CPU between resource samples. These values are not standalone server or agent measurements.
- PostgreSQL container memory peaked at 426.7 MiB across 55 resource samples; no resource-sampling errors occurred.

Evidence: [workload distributions](validation/agent-soak-2026-09-27.json), [resource samples](validation/agent-soak-resources-2026-09-27.json), and [hardware, database and artifact details](validation/agent-soak-environment-2026-09-27.json). The one-minute smoke and ten-minute run both passed. This closes the agreed local ten-minute throughput/recovery check; retained-history storage sizing and longer deployment validation remain separate gates.

Run against a separate empty database using a PostgreSQL role allowed to execute `CHECKPOINT`. The wrapper samples native process RSS/CPU and Docker PostgreSQL statistics about every ten seconds. Native measurements include the 500 simulated clients; Docker statistics include other databases in the same container. The wrapper never deletes a database or overwrites an existing workload report.

```sh
HOPE_SOAK_DATABASE_URL=postgres://USER:PASSWORD@HOST/EMPTY_DISPOSABLE_DB \
  python3 scripts/agent-soak-test.py \
    --postgres-container YOUR_POSTGRES_CONTAINER \
    --output /tmp/hope-soak-results
```

Set `HOPE_SOAK_ROUNDS=240` for one hour, or up to `1440` for six hours. Estimate disk requirements before extending the run; the harness preserves its fixture for inspection. A four-round smoke run checks TLS, ingestion and retention, but ends before reconnect cycles begin.

Longer-term production sizing still needs representative retained history, production network conditions and multi-hour reconnect cycles. The local passes should not be presented as a guarantee for every 500-agent installation.

## Storage planning at 500 agents

Agent count alone is insufficient for a storage claim. At the normal workload of one log per second per agent, 500 agents generate 43.2 million logs per day. A sample during the TLS run measured about 421 bytes per message plus attributes: approximately 119 GiB of uncompressed payload over seven days, before identities, timestamps, row overhead, indexes and WAL. Repeating the tenfold burst every 150 seconds increases the mean rate to 1.9 logs per second per agent and raises that projection further.

The 1,476-byte metric payload at 15-second intervals adds 2.88 million samples per day, or approximately 28 GiB of serialized metric payload over seven days, excluding rollups and database overhead. These are workload-specific payload projections, not measured retained database sizes; compression, table layout and write amplification change physical allocation.

The reference host had about 46 GiB free before the sustained run. A short throughput pass therefore cannot certify the default seven-day retention window at this traffic rate on the current disk. Provision a measured storage budget or agree lower source rates/retention before deploying 500 similarly active agents. This validation does not silently change collection or retention settings.
