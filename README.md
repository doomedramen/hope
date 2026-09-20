# hope

Hope is a homelab operations platform for network discovery, monitoring,
maintenance planning, notifications, and managed agents. The current backend
includes the M1–M9 inventory, discovery, monitoring, agent, release-update,
dependency, alert-suppression, and maintenance slices. The M9 maintenance API
and calendar/timeline/list UI are available.

See [`docs/adr/`](docs/adr/) for architecture decisions and
[`docs/threat-model.md`](docs/threat-model.md) for current controls and gaps.

## Quickstart with Docker Compose

Prerequisites: Docker Engine with Compose v2, `openssl`, and `curl`.

The example exposes the API as plain HTTP on `127.0.0.1:8080` for local use.
It also exposes the server-terminated agent TLS listeners on ports 8443 and
8444. Do not expose the HTTP listener directly to an untrusted network; use a
TLS reverse proxy for any non-local deployment. See
[`docs/operations/backup-restore.md`](docs/operations/backup-restore.md) and
[`docs/operations/upgrade-recovery.md`](docs/operations/upgrade-recovery.md)
before using the stack for persistent data.

The canonical Compose file uses the published server/worker image from GHCR.
Set `HOPE_IMAGE` in `.env` to select another image. For production, use an
immutable digest. Contributors can use the local-build override documented
below.

### 1. Configure local state

```sh
git clone https://github.com/doomedramen/hope.git
cd hope
cp .env.example .env

# Set one real password in both POSTGRES_PASSWORD and HOPE_DATABASE_URL.
mkdir -p deploy/compose/secrets deploy/compose/agent-releases
umask 077
test -e deploy/compose/secrets/credential-master-key || \
  openssl rand -hex 32 > deploy/compose/secrets/credential-master-key

# Local quickstart has no agent release yet. Add a real public key before
# enabling signed agent install or update operations.
test -e deploy/compose/agent-releases/public-key.hex || \
  : > deploy/compose/agent-releases/public-key.hex
```

For local HTTP, keep `HOPE_COOKIE_SECURE=false`. For production, set it to
`true`, change both agent URLs to the public service names, and put a TLS
reverse proxy in front of port 8080. The 8443 mTLS gateway must use TCP
passthrough; the 8444 enrollment listener should also preserve the server
certificate and CA fingerprint. See
[`docs/manual-agent-install.md`](docs/manual-agent-install.md).

### 2. Select an image and initialise server PKI

The default `.env.example` value uses the published `main` image. Pull it
before the first start:

```sh
docker compose -f deploy/compose/docker-compose.yml --env-file .env pull server worker
docker compose -f deploy/compose/docker-compose.yml --env-file .env \
  run --rm --no-deps server ca init
```

`ca init` writes the internal CA key and certificate plus the server leaf
certificate into the `server-pki` volume. It refuses to overwrite existing PKI
files. Back up this volume before changing deployments.

To build the server/worker image from the current checkout, add the explicit
local-build override to every Compose command:

```sh
docker compose \
  -f deploy/compose/docker-compose.yml \
  -f deploy/compose/docker-compose.local-build.yml \
  --env-file .env build server worker
docker compose \
  -f deploy/compose/docker-compose.yml \
  -f deploy/compose/docker-compose.local-build.yml \
  --env-file .env run --rm --no-deps server ca init
docker compose \
  -f deploy/compose/docker-compose.yml \
  -f deploy/compose/docker-compose.local-build.yml \
  --env-file .env up -d
```

Use the same two `-f` options with later `ps`, `logs`, and `down` commands.

GHCR packages can be public or private. Public packages need no registry
login. For a private package, run `docker login ghcr.io -u YOUR_GITHUB_USERNAME`
and enter a GitHub token with `read:packages` when prompted. GitHub Actions uses
the built-in `GITHUB_TOKEN` with package write permission.

### 3. Start and check the stack

```sh
docker compose -f deploy/compose/docker-compose.yml --env-file .env up -d
docker compose -f deploy/compose/docker-compose.yml --env-file .env ps

curl -fsS http://127.0.0.1:8080/health/live
# {"status":"live"}
curl -fsS http://127.0.0.1:8080/health/ready
# {"status":"ready"}
```

`server` serves the API and web SPA, applies embedded SQL migrations, and runs
the agent gateway and enrollment listeners. `worker` applies the same
migrations, consumes the PostgreSQL job queue, and runs monitor checks and
background handlers. The worker may start independently after PostgreSQL is
healthy; Compose does not treat server startup as a worker dependency.

### Use another image or an immutable digest

Set `HOPE_IMAGE` in `.env`, then pull and start the selected image:

```sh
# Fork or rebuilt image.
export HOPE_IMAGE=ghcr.io/YOUR_GITHUB_USERNAME/hope:main

# Or use a production pin. Replace DIGEST with the published sha256 digest.
# export HOPE_IMAGE=ghcr.io/doomedramen/hope@sha256:DIGEST

docker compose -f deploy/compose/docker-compose.yml --env-file .env pull server worker
docker compose -f deploy/compose/docker-compose.yml --env-file .env up -d
```

The export overrides `HOPE_IMAGE` from `.env` for this shell. To persist the
choice, put only one of these values in `.env` instead. A one-command shell
override can also be supplied:

```sh
HOPE_IMAGE=ghcr.io/YOUR_GITHUB_USERNAME/hope:main \
  docker compose -f deploy/compose/docker-compose.yml --env-file .env up -d
```

Create the first operator account:

```sh
curl -fsS -X POST http://127.0.0.1:8080/api/v1/setup \
  -H 'content-type: application/json' \
  -H 'x-requested-with: hope' \
  -d '{"email":"admin@example.com","password":"correcthorsebatterystaple"}'

curl -fsS -X POST http://127.0.0.1:8080/api/v1/login \
  -H 'content-type: application/json' \
  -H 'x-requested-with: hope' \
  -c cookies.txt \
  -d '{"email":"admin@example.com","password":"correcthorsebatterystaple"}'
```

Unsafe `/api/v1` requests require the `X-Requested-With: hope` header. The
session cookie is stored server-side in PostgreSQL.

### 4. Inspect worker activity

```sh
docker compose -f deploy/compose/docker-compose.yml --env-file .env logs -f worker
```

The server scheduler enqueues idempotent work immediately and every five
minutes. Current periodic work includes agent health, automatic update
reconciliation, dependency-graph reconciliation, maintenance reconciliation,
and due discovery change scans. Session cleanup and enrollment-token purge run
hourly. Monitor-result and change-event retention run daily. The worker also
handles monitor checks, notifications, discovery, agent install/repair, signed
agent updates, and maintenance-overrun delivery. Failed retryable jobs use
queue backoff; unknown job kinds fail permanently because an older worker
cannot safely process a newer job contract.

## M9 maintenance API

All maintenance routes require an authenticated operator session and the CSRF
header for mutations:

- `GET`/`POST /api/v1/maintenance-events` list or create events.
- `GET`/`PATCH /api/v1/maintenance-events/{id}` inspect or edit an event.
- `GET /api/v1/maintenance-events/{id}/conflicts` inspect conflict reasons.
- `POST /api/v1/maintenance-events/{id}/start` start an event.
- `POST /api/v1/maintenance-events/{id}/complete` complete an event.
- `POST /api/v1/maintenance-events/{id}/cancel` cancel an event.

An event uses an IANA timezone, a start and end (or duration), optional RFC
5545 recurrence, lead-in and cooldown reservations, and normalized resources
with `target`, `required`, `affected`, or `exclusive` roles. Recurrence expands
through a 90-day horizon by default; `HOPE_MAINTENANCE_HORIZON_DAYS` may set a
value from 1 through 366. Ambiguous and nonexistent daylight-saving local
times are rejected.

Creates and edits check shared resources, confirmed dependency paths, affected
services, and the default global disruptive-event lock in one transaction.
Conflicts return the overlap and a later-start suggestion. Edits and lifecycle
changes use an optimistic `version`; send the current version or receive a
conflict. Only draft, scheduled, and upcoming events are editable.

The worker advances events through `scheduled`, `upcoming`, `active`,
`overrunning`, and `completed`. A past end with an open expected incident
becomes `overrunning`, retains its reservation, and queues an idempotent
`maintenance.overrun` notification through configured webhook or ntfy routes.
Expected-failure suppression records retain the exact event and occurrence;
the incident and recovery state remain durable.

## Local development

See [`justfile`](justfile) for all recipes:

```sh
just dev       # server; uses HOPE_DATABASE_URL
just web       # Vite development server
just migrate   # apply embedded migrations and exit
just test      # Rust and web tests
just lint      # formatting, Clippy, and web lint
```

DB-gated tests need a disposable PostgreSQL 17 instance and `DATABASE_URL`.

The ignored Docker discovery acceptance gate needs both `DATABASE_URL` and a
reachable Docker daemon. It starts and removes a disposable `nginx:1.27-alpine`
container, then runs the real worker scanner and classifier against it:

```sh
DATABASE_URL=postgres://postgres:dev@localhost:55432/postgres \
  cargo test -p server m2_docker_high_port_scan_creates_canonical_inventory_records \
  -- --ignored --nocapture --test-threads=1
```

The gate skips only when Docker is unavailable. It fails for missing database
configuration, image pulls, scan failures, or record mismatches.

## Operations and release documentation

- [Backup and clean restore](docs/operations/backup-restore.md)
- [Upgrade and recovery](docs/operations/upgrade-recovery.md)
- [Signed agent releases](docs/release-signing.md)
- [Manual agent installation](docs/manual-agent-install.md)
- [Threat model](docs/threat-model.md)
- [Contributing](CONTRIBUTING.md)
- [Security reporting](SECURITY.md)
- [Release process](RELEASE.md)

To stop local containers while preserving database and PKI volumes:

```sh
docker compose -f deploy/compose/docker-compose.yml --env-file .env down
```

`down -v` removes the PostgreSQL and server-PKI volumes. Use it only when
intentionally discarding local state or following a verified clean-restore
procedure.
