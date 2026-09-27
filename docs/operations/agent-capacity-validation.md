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

Next production gate: repeat on agreed deployment hardware with realistic full payloads and retained history, TLS, normal and burst log rates, sustained concurrent retention/checkpoint load, slow consumers, and multi-hour reconnect cycles. Record process/cgroup CPU and memory, disk allocation, WAL, latency distributions and recovery time. The short local passes should not be presented as a guarantee for every 500-agent installation.
