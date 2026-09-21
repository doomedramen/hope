# Proxmox VE installation and updates

The Helper Script deploys Hope in its own unprivileged Debian 13 LXC. Docker
Engine and Compose run inside that LXC; nothing is installed into the Proxmox
host beyond the container managed by the Helper Script.

The current published image is `linux/amd64`, so use an amd64 Proxmox host.
The default container has 2 vCPUs, 4 GiB RAM, and a 16 GiB disk. Adjust those
values in the Helper Script prompts for larger inventories or longer
retention.

## Install

Run this in a Proxmox VE host shell:

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/doomedramen/hope/main/ct/hope.sh)"
```

Open `http://<container-ip>` after installation. HTTP is published on port 80
because the LXC has its own IP address. The first-run page creates the operator
account.

The installer writes these managed files:

```text
/opt/hope/.env
/opt/hope/deploy/compose/docker-compose.yml
/opt/hope/deploy/compose/backup.sh
/opt/hope/deploy/compose/restore.sh
/usr/local/sbin/hope-update
```

The `.env` file is root-readable only. It contains the generated PostgreSQL
password and deployment settings. Keep it with the backup set.

The default `HOPE_IMAGE` tracks `ghcr.io/doomedramen/hope:main`. Set it to a
reviewed version tag or immutable digest in `/opt/hope/.env` before an update
when a pinned rollout is required.

## Update

Back up first. The update applies forward-only database migrations and does
not create a backup automatically:

```sh
HOPE_BACKUP_INCLUDE_SECRETS=1 \
  /opt/hope/deploy/compose/backup.sh /opt/hope/backups
```

Enter the Hope LXC, then run the same Helper Script command:

```sh
pct enter <CTID>
bash -c "$(curl -fsSL https://raw.githubusercontent.com/doomedramen/hope/main/ct/hope.sh)"
```

Inside an existing Hope LXC, the script takes its update path instead of
creating another container. It refreshes the managed Compose and backup files,
pulls only the Hope server/worker image, and compares the image IDs before and
after the pull. If the image is already current, it exits successfully and
leaves the running services untouched. Otherwise it stops writers, starts
PostgreSQL, runs `server migrate`, waits for readiness, then starts the worker.

`/usr/local/sbin/hope-update` performs the same flow without downloading the
outer Helper Script. Prefer the Helper Script command for normal updates so
the latest updater is used.

Do not run an older Hope image after an update that may have applied a
migration. Follow [Upgrade and recovery](upgrade-recovery.md) for a
forward-fix or clean-restore path.

## Configuration and agents

Edit `/opt/hope/.env` for image pins and HTTP/HTTPS bind settings. Run the
update command after changing `HOPE_IMAGE`; for other Compose
settings, restart the stack with:

```sh
docker compose --project-name hope --env-file /opt/hope/.env \
  -f /opt/hope/deploy/compose/docker-compose.yml up -d
```

Signed agents and release trust ship inside the image. Hope listens on host
ports 80 and 443; no release directory or other inbound port is required.
On the Agents page, set the connection address to the LXC's HTTP origin or
your public HTTPS Proxy Host. See [Manual agent installation](../manual-agent-install.md).

Never use `docker compose down -v` unless intentionally discarding the Hope
database and server PKI.
