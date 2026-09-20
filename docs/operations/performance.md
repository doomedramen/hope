# M10 performance test

M10 defines a v1 homelab sizing boundary. It does not define a production
capacity guarantee. The boundary is up to 1,000 devices, 10,000 services or
checks, and 500 agents, with monitoring intervals from seconds to minutes.

`apps/server/tests/m10_scale.rs` provides a repeatable database and scheduler
claim harness for this boundary. The live test is ignored by default because it
needs PostgreSQL and creates a large fixture. The normal test command runs a
small deterministic smoke path that checks the dataset plan and percentile
calculation without a database.

## What test measures

The live harness creates one isolated PostgreSQL schema, applies all embedded
migrations, and inserts this synthetic dataset:

- 1,000 active devices;
- 10,000 services and 10,000 endpoints;
- 9,500 TCP monitors for services;
- 500 agent-heartbeat monitors for 500 agents;
- 500 current agent inventory snapshots;
- 10,000 pending jobs in the PostgreSQL job queue.

It runs five warmups, then 25 sequential samples for each path. It measures:

1. `monitor_claim_transaction`: pool acquire, transaction begin, the same
   due-monitor claim SQL used by the monitor scheduler, and transaction
   rollback. The rollback keeps each sample repeatable.
2. `job_claim_transaction`: the existing `jobs::claim` path, including its
   transaction, `FOR UPDATE SKIP LOCKED` selection, lease update, and commit.
   The claimed row is reset outside the timed section.

Acceptance targets are harness regression thresholds for the documented test
setup, not service-level objectives:

- monitor claim p95: <= 250 ms;
- job claim p95: <= 250 ms.

The harness prints sample count, minimum, p50, p95, maximum, target, database
version, `shared_buffers`, and `max_connections`. A failed target fails the
ignored live test. Dataset loading and `ANALYZE` are not included in latency
samples.

## What test does not measure

This test does not measure network check time, TCP/HTTP/DNS/TLS behavior,
agent WebSocket connections, agent CPU or memory use, API latency, UI latency,
concurrent scheduler workers, connection-pool contention, retention or
90-day history growth, notification delivery, discovery scans, disk failure,
or restart behavior. It does not prove that production can sustain the M10
boundary. Use a separate workload test before changing deployment capacity.

## Prerequisites

- Rust toolchain used by repository CI;
- PostgreSQL with the repository migrations supported, preferably the same
  PostgreSQL major version used by the release candidate;
- permission to create and drop a schema and to install or use `pgcrypto`;
- a throwaway database, or a database where this isolated benchmark schema is
  acceptable;
- enough disk and memory for roughly 10,000 rows in each main fixture table.

The harness uses `M10_DATABASE_URL` first and accepts `DATABASE_URL` as a
fallback. It never drops `public`; it creates a unique `m10_scale_*` schema and
drops that schema after successful setup and measurement. A failed process can
leave its uniquely named schema. Remove only that exact schema after checking
that no run still uses it, or discard the throwaway database.

## Run smoke checks

From repository root:

```sh
cargo test -p server --test m10_scale
cargo fmt --all -- --check
cargo check -p server --tests
```

The first command runs the deterministic smoke test. It does not run the
ignored PostgreSQL test.

## Run live boundary test

Use a separate PostgreSQL database. Example with PostgreSQL 17:

```sh
docker run --rm --name hope-m10-postgres \
  -e POSTGRES_PASSWORD=dev \
  -p 55432:5432 -d postgres:17
```

Wait for PostgreSQL readiness, then run from repository root in another shell:

```sh
M10_DATABASE_URL=postgres://postgres:dev@127.0.0.1:55432/postgres \
  ./scripts/m10-scale-test.sh
```

Equivalent direct command:

```sh
M10_DATABASE_URL=postgres://postgres:dev@127.0.0.1:55432/postgres \
  cargo test --release -p server --test m10_scale -- \
  --ignored --nocapture --test-threads=1
```

Run with application server and worker stopped when using a database shared
with any development deployment. The harness uses one database connection and
does not represent concurrent worker behavior.

## Release-candidate record

Run on hardware that represents intended homelab deployments. Record the raw
output; do not replace it with a pass/fail claim. Keep the record with release
artifacts or the release change log.

Hardware is an input to this result, not a fixed property of the harness. The
record must identify CPU, memory, storage, operating system, and database host
so results from different machines are not treated as one measurement.

```sh
mkdir -p /tmp/hope-m10-results
M10_DATABASE_URL=postgres://postgres:dev@127.0.0.1:55432/postgres \
  ./scripts/m10-scale-test.sh 2>&1 | \
  tee /tmp/hope-m10-results/$(date -u +%Y%m%dT%H%M%SZ).txt
```

Record these fields with the output:

```text
release version and commit:
test date (UTC):
host CPU and core count:
host memory:
storage type and filesystem:
operating system and kernel:
PostgreSQL version:
PostgreSQL shared_buffers:
PostgreSQL max_connections:
database host and network path:
other database clients or workers running:
```

Compare results only when dataset counts, PostgreSQL major version, host class,
and test command match. Investigate target failures with query plans, database
logs, and host load before changing thresholds. Threshold changes require a
documented reason and a new release-candidate run.
