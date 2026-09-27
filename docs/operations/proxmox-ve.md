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

## Discover VMs and LXCs from a Proxmox host

Install a current Hope agent directly on each Proxmox VE host using the
[manual agent installer](../manual-agent-install.md). An agent inside the Hope
LXC sees that container, not its parent Proxmox host. Deploy the server/web
update and migration 0042 before updating the host agents.

On a Proxmox host, the installer adds `hope-agent-proxmox.service` and
`hope-agent-proxmox.timer`. Existing installations must rerun the installer
once after updating to an agent that supports `export-proxmox`; replacing the
binary alone does not create these units. Enrollment is reused.

The root oneshot runs two fixed, read-only `pvesh get` requests against the
local node's QEMU and LXC inventories. It exports only VMID, name, type,
reported power state, template flag, and basic resource metadata into
`/run/hope-proxmox/inventory.json`. It does not export guest configurations,
passwords, tokens, or other nodes' inventory. Proxmox discovery grants the
network-facing agent no additional groups or sudo rights. If Docker also runs
on that host, the installer's separate [Docker access setup](../manual-agent-install.md#docker-inventory-access)
can grant root-equivalent Docker access unless explicitly skipped. Proxmox's
[local API shell](https://pve.proxmox.com/pve-docs/pvesh.1.html) requires root;
the separate service keeps that permission outside the network-facing process.

The helper refreshes every minute. The agent sends this inventory on connect
and with its normal 15-minute inventory snapshots. Devices shows VM/LXC
counts and observation freshness; opening the host shows its guest list.
Guest power states are observations, not live availability checks. Host
metrics remain available through the agent's Metrics link.

Each guest is a workload with a `host_device_id` foreign key and containment
edge. VMIDs are scoped to the reporting host, so identical IDs on independent
nodes do not collide. Repeated snapshots update the same record. Host
merge/undo keeps these relationships. A complete report can mark a missing
guest as no longer present; partial, stale, or failed reports retain previous
records and show unknown current state. At most 128 guests are exported; a
larger inventory is explicitly partial.

This first phase does not correlate guest-installed agents, preserve a guest's
identity across live migration, or provide cluster/storage management. A
migrated guest appears on its new host while its old host retains history.
No guest start/stop or configuration actions are exposed.

Troubleshoot missing inventory on the **Proxmox host**:

```sh
systemctl status hope-agent-proxmox.timer hope-agent-proxmox.service --no-pager
journalctl -u hope-agent-proxmox.service --since '30 minutes ago' --no-pager
sudo -u hope-agent cat /run/hope-proxmox/inventory.json
```

After correcting a helper failure, `systemctl start hope-agent-proxmox.service`
refreshes the local cache. Restart `hope-agent` to send a new snapshot
immediately, or wait for its next inventory interval.
