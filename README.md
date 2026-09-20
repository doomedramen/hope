# hope

Hope is a homelab operations platform for network discovery, monitoring,
maintenance planning, notifications, and managed agents. The current backend
includes the M1–M9 inventory, discovery, monitoring, agent, release-update,
dependency, alert-suppression, and maintenance slices. The M9 maintenance API
and calendar/timeline/list UI are available.

See [`docs/adr/`](docs/adr/) for architecture decisions and
[`docs/threat-model.md`](docs/threat-model.md) for current controls and gaps.

## Docker Compose

The web UI is bundled into the server image, so there is no frontend container.
The worker uses the same image with a different command. The stack needs only
Docker Engine with Compose v2.

Save this as `docker-compose.yml`, or use the identical file at
`deploy/compose/docker-compose.yml`:

```yaml
services:
  postgres:
    image: postgres:17
    restart: unless-stopped
    environment:
      POSTGRES_DB: ${POSTGRES_DB:-hope}
      POSTGRES_USER: ${POSTGRES_USER:-hope}
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:-hope}
    volumes:
      - postgres-data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U $${POSTGRES_USER} -d $${POSTGRES_DB}"]
      interval: 5s
      timeout: 5s
      retries: 12
      start_period: 10s

  server:
    image: ${HOPE_IMAGE:-ghcr.io/doomedramen/hope:main}
    restart: unless-stopped
    depends_on:
      postgres:
        condition: service_healthy
    healthcheck:
      test: ["CMD", "curl", "--fail", "http://localhost:8080/health/ready"]
      interval: 30s
      timeout: 5s
      retries: 5
      start_period: 15s
    environment:
      HOPE_DATABASE_URL: ${HOPE_DATABASE_URL:-postgres://${POSTGRES_USER:-hope}:${POSTGRES_PASSWORD:-hope}@postgres:5432/${POSTGRES_DB:-hope}}
      HOPE_COOKIE_SECURE: ${HOPE_COOKIE_SECURE:-false}
      HOPE_AGENT_ENROLL_URL: ${HOPE_AGENT_ENROLL_URL:-https://localhost:8444}
      HOPE_AGENT_GATEWAY_URL: ${HOPE_AGENT_GATEWAY_URL:-wss://localhost:8443}
    ports:
      - "${HOPE_HTTP_BIND:-0.0.0.0}:${HOPE_HTTP_PORT:-8080}:8080"
      - "${HOPE_GATEWAY_BIND:-0.0.0.0}:${HOPE_GATEWAY_PORT:-8443}:8443"
      - "${HOPE_ENROLL_BIND:-0.0.0.0}:${HOPE_ENROLL_PORT:-8444}:8444"
    volumes:
      - server-pki:/app/data/pki
      - ${HOPE_AGENT_RELEASE_DIR_HOST:-./agent-releases}:/app/agent-releases:ro
    command: ["serve"]

  worker:
    image: ${HOPE_IMAGE:-ghcr.io/doomedramen/hope:main}
    restart: unless-stopped
    depends_on:
      postgres:
        condition: service_healthy
      server:
        condition: service_healthy
    environment:
      HOPE_DATABASE_URL: ${HOPE_DATABASE_URL:-postgres://${POSTGRES_USER:-hope}:${POSTGRES_PASSWORD:-hope}@postgres:5432/${POSTGRES_DB:-hope}}
    volumes:
      - server-pki:/app/data/pki:ro
      - ${HOPE_AGENT_RELEASE_DIR_HOST:-./agent-releases}:/app/agent-releases:ro
    command: ["worker"]

volumes:
  postgres-data:
  server-pki:
```

Start it:

```sh
docker compose -f deploy/compose/docker-compose.yml up -d
```

Open <http://localhost:8080>. On an empty database, the web UI presents the
first-run form for creating the operator account. No account, key, certificate,
or manual migration command is required. The server image creates the
credential key and agent CA on first start and keeps them in the `server-pki`
volume. PostgreSQL migrations run when the server and worker start.

For a source build, use the checked-in override:

```sh
docker compose \
  -f deploy/compose/docker-compose.yml \
  -f deploy/compose/docker-compose.local-build.yml \
  up -d --build
```

The HTTP port is published on all interfaces by default for homelab use. Set
`HOPE_HTTP_BIND=127.0.0.1` for a local-only listener. Put a TLS reverse proxy
in front of HTTP for an internet-facing deployment, set
`HOPE_COOKIE_SECURE=true`, use a strong PostgreSQL password, and pin `HOPE_IMAGE`
to an immutable GHCR digest. Set the agent URLs to the hostname reachable by
agents. The 8443 gateway needs TCP passthrough and 8444 must preserve the
server certificate and CA fingerprint.

Copy `.env.example` to `.env` only when changing those defaults. Public images
need no registry login. For a private GHCR package, log in with a GitHub token
that has `read:packages`.

To stop the stack while preserving data:

```sh
docker compose -f deploy/compose/docker-compose.yml down
```

`down -v` removes the PostgreSQL and server-PKI volumes. Use it only when
intentionally discarding the deployment.

## Proxmox VE Helper Script

Install Hope into a dedicated Debian 13 LXC from a Proxmox VE host:

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/doomedramen/hope/main/ct/hope.sh)"
```

The helper creates an unprivileged, Docker-enabled LXC, installs the published
Hope image and PostgreSQL stack, and writes its deployment to `/opt/hope`.
It currently supports `amd64` Proxmox hosts. See
[Proxmox VE installation and updates](docs/operations/proxmox-ve.md) for
resource defaults, backups, configuration, and the safe update command.

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
- [Proxmox VE installation and updates](docs/operations/proxmox-ve.md)
- [Signed agent releases](docs/release-signing.md)
- [Manual agent installation](docs/manual-agent-install.md)
- [Threat model](docs/threat-model.md)
- [Contributing](CONTRIBUTING.md)
- [Security reporting](SECURITY.md)
- [Release process](RELEASE.md)
