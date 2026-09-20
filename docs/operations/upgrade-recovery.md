# Upgrade and recovery

## Compatibility policy

Hope uses forward-only embedded SQL migrations. `server serve`, `server
migrate`, and `worker` run the `sqlx::migrate!` set from the image. Migration
files are applied in order and are not automatically downgraded.

The operational target is two supported application versions: the current
release (`N`) and the immediately previous release (`N-1`). This is a support
target, not an implemented rolling-upgrade guarantee. The repository does not
yet prove every `N-1`/`N` pair with a compatibility matrix, and the Compose
example runs server and worker from one image. Use matching server and worker
images from one release.

Do not assume that an arbitrary old worker can process jobs from a newer
server. Unknown job kinds fail permanently. Do not mix versions during a
rolling deployment unless the release notes and a tested compatibility result
explicitly allow that pair. There is no supported promise for direct upgrades
from `N-2` or older; use sequential upgrades or a clean restore and follow the
release-specific instructions.

The two-version target does not promise:

- automatic schema rollback;
- zero downtime;
- compatibility of every undocumented internal job payload;
- recovery of a lost credential key, CA key, signing key, or release bundle; or
- agent binary compatibility beyond the signed protocol floor checked by the
  release repository and agent.

## Before upgrading

1. Read the release notes, migration files, and
   [`RELEASE.md`](../../RELEASE.md).
2. Confirm server and worker will use the same release commit or immutable
   image digest.
3. Confirm the PostgreSQL backup, credential master key, exact `server-pki`
   volume, release repository, and release trust configuration are backed up.
   Follow [Backup and clean restore](backup-restore.md).
4. Record current health, running image or Git revision, and any pending jobs.
5. Confirm the reverse proxy and agent endpoints still point at the deployment
   being upgraded.

Do not start a migration against the only copy of production data. Keep the
backup until the new release passes its checks and the retention policy allows
expiry.

## Compose migration procedure

The procedure below keeps the database up while stopping application writers.
It makes migration failure visible before server and worker restart.

```sh
docker compose -f deploy/compose/docker-compose.yml --env-file .env stop worker server

# Build or pull the exact same release image for both services.
docker compose -f deploy/compose/docker-compose.yml --env-file .env build server worker

docker compose -f deploy/compose/docker-compose.yml --env-file .env up -d postgres
docker compose -f deploy/compose/docker-compose.yml --env-file .env ps postgres

docker compose -f deploy/compose/docker-compose.yml --env-file .env \
  run --rm --no-deps server migrate

docker compose -f deploy/compose/docker-compose.yml --env-file .env up -d server
curl -fsS http://127.0.0.1:8080/health/live
curl -fsS http://127.0.0.1:8080/health/ready

docker compose -f deploy/compose/docker-compose.yml --env-file .env up -d worker
docker compose -f deploy/compose/docker-compose.yml --env-file .env ps
docker compose -f deploy/compose/docker-compose.yml --env-file .env logs --tail=200 server worker
```

`server migrate` needs the same database URL, secret mount, release trust
configuration, and image as the service. It does not start the API. The
worker also runs migrations on startup because it can start independently after
PostgreSQL is healthy; the SQL migration lock prevents concurrent application.

After the first health checks, verify login, one read-only inventory request,
one maintenance-event list request, worker job claims, notification queue
delivery, and agent gateway reconnection. Do not use an agent update as the
first upgrade smoke test.

## Rollback boundary

There are two different cases:

### Image changed, migration not applied

Stop the new server and worker, restore the previous image or checkout, and
start the previous pair. This is the simple binary rollback case. Preserve the
database and inspect logs before retrying.

### Migration applied

Do not start the old image automatically. This project has no down-migration
procedure and makes no backward-schema guarantee for an arbitrary release.
Choose one of these reviewed recovery paths:

- deploy a forward-fix release that understands the migrated schema; or
- restore the pre-upgrade PostgreSQL backup into a clean Compose project,
  restore the matching credential master key, server PKI, release repository,
  and trust configuration, then run the previous matching server and worker
  images.

Restoring only PostgreSQL is incomplete. Restoring only the binary is not a
database rollback. Use the clean restore procedure when state and binary must
return to a known pair.

Agent update rollback is separate from control-plane rollback. The worker can
restore an agent's previous binary when that agent fails its update check-in,
but that does not undo a PostgreSQL migration or restore server PKI. See
[`docs/release-signing.md`](../release-signing.md) and
[`docs/manual-agent-install.md`](../manual-agent-install.md).

## Recovery after a failed upgrade

1. Stop server and worker if they are repeatedly failing or enqueueing unknown
   job kinds.
2. Save logs and the migration error. Do not delete the database volume.
3. Confirm which migrations are recorded in `sqlx` migration history before
   choosing image rollback or clean restore.
4. Restore the exact key material and release trust files if any mount or
   secret changed during the attempt.
5. Use a forward-fix release when the database is already migrated. Use clean
   restore when the previous release must run against the pre-upgrade schema.
6. Re-run readiness, login, worker, maintenance, notification, and agent
   reconnection checks before reopening traffic.

If migration status is uncertain, treat the database as migrated. A failed
migration may have committed earlier migration steps; do not infer database
state from container exit status alone.

## Version support record

For each release, record:

- release version and immutable source/image identifier;
- migration range present in the image;
- tested previous-version pair (`N-1`, if available);
- whether server and worker can be mixed during upgrade;
- agent protocol floor and release trust-key changes; and
- restore test date and backup format.

Until this record and compatibility tests exist for a pair, operate it as a
lockstep upgrade with a tested backup and a forward-recovery plan.
