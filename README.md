# hope

A homelab operations platform: discovery, monitoring, maintenance, and
agent management for a home network. See `spec.md` for the full product
and engineering specification, and `docs/adr/` for architecture decisions.

This is Milestone 0 (spec §17): repository and engineering foundation.
No real discovery/monitoring/maintenance features exist yet.

## Quickstart (Docker Compose)

Prerequisites: Docker with Compose v2.

```sh
git clone <this repo> && cd hope
cp .env.example .env
# edit .env: set a real POSTGRES_PASSWORD, and update HOPE_DATABASE_URL
# to match it (same password, same DB name).
```

Build the images first (the server image also builds the web SPA):

```sh
docker compose -f deploy/compose/docker-compose.yml --env-file .env build
```

Generate the internal CA and server TLS cert (ADR-0007) — required before
`server` can start its agent gateway/enroll listeners. This writes into a
named volume (`server-pki`) shared by the `server` service, so it only
needs to be done once per deployment:

```sh
docker compose -f deploy/compose/docker-compose.yml --env-file .env run --rm server ca init
```

Start the stack:

```sh
docker compose -f deploy/compose/docker-compose.yml --env-file .env up -d
```

This starts three services: `postgres`, `server` (API + web UI + agent
gateway/enroll listeners; applies migrations on startup), and `worker`
(background job queue consumer — same image, `worker` command).

Verify:

```sh
# postgres healthy, migrations applied, server ready:
curl -f http://localhost:8080/health/ready
# -> {"status":"ready"}

# SPA loads:
curl -sf http://localhost:8080/ | grep -o '<title>[^<]*</title>'
# -> <title>Hope</title>

# create the admin account (first-run flow, spec §13.2). The
# X-Requested-With header is required on mutating /api/v1 requests
# (CSRF defense, spec §12.3 — see apps/server/src/csrf.rs):
curl -sf -X POST http://localhost:8080/api/v1/setup \
  -H 'content-type: application/json' -H 'x-requested-with: hope' \
  -d '{"email":"admin@example.com","password":"correcthorsebatterystaple"}'

# log in:
curl -sf -X POST http://localhost:8080/api/v1/login \
  -H 'content-type: application/json' -H 'x-requested-with: hope' \
  -c cookies.txt \
  -d '{"email":"admin@example.com","password":"correcthorsebatterystaple"}'
```

Check the worker is actually processing jobs (it runs scheduled
maintenance jobs — expired-session cleanup, expired/used enrollment-token
purge — every few minutes; see `apps/server/src/scheduler.rs`):

```sh
docker compose -f deploy/compose/docker-compose.yml --env-file .env logs worker
```

Tear down (removes containers, network, *and volumes* — i.e. the database
and generated CA):

```sh
docker compose -f deploy/compose/docker-compose.yml --env-file .env down -v
```

## Local development (without Docker)

See `justfile` for the full list of recipes (`just --list`). Broad
strokes: `just dev` runs the server against whatever `HOPE_DATABASE_URL`
points to, `just web` runs the SPA dev server, `just test` runs the full
test suite (DB-gated tests are skipped unless `DATABASE_URL` is set —
run them with a throwaway `docker run --rm -d -p 55432:5432 -e
POSTGRES_PASSWORD=dev postgres:17` and `DATABASE_URL=postgres://postgres:dev@localhost:55432/postgres
just test`).

### M2 Docker discovery acceptance gate

Run this ignored gate explicitly. It needs `DATABASE_URL` and a reachable
Docker daemon; normal unit tests never require Docker. The gate starts a
disposable `nginx:1.27-alpine` HTTP container, lets Docker assign a random
high host port on a private local interface, runs the real worker scanner and
classifier against that address, repeats the scan, and removes the container
on exit:

```sh
DATABASE_URL=postgres://postgres:dev@localhost:55432/postgres \
  cargo test -p server m2_docker_high_port_scan_creates_canonical_inventory_records \
  -- --ignored --nocapture --test-threads=1
```

The gate skips only when Docker is unavailable. It fails for missing database
configuration, image pulls, scan failures, or record mismatches. It checks an
open-port observation, HTTP protocol classification, one canonical socket
endpoint and service across the repeat scan, per-run evidence, and one
classification event.

Release signing (agent binaries, spec §7.5/§7.7): see
`docs/release-signing.md`.

## Threat model

See `docs/threat-model.md` — current mitigations and known gaps.
