# Operations

Operator guide for H.O.P.E. It documents behavior that is implemented and tested in this repository; where a deployment path has not been exercised, the text says so. The evidence behind each claim is in [validation.md](validation.md), and the design is in [architecture.md](architecture.md). Release status: pre-release, see the gate table in `validation.md`.

Contents: [Installing the server](#installing-the-server), [Canonical URL and reverse proxy](#canonical-url-and-reverse-proxy), [First-run setup](#first-run-setup), [Custom CA](#custom-ca-server-agent-and-ssh-destination), [Discovery](#discovery-initial-scans-and-monitoring), [Capability limits](#capability-limitations), [Probe traffic](#probe-traffic), [Identity reconciliation](#identity-reconciliation), [Installing agents](#installing-agents), [Upgrade, rollback, uninstall](#agent-upgrade-rollback-and-uninstall), [Updating from the console](#updating-agents-from-the-console), [Revocation](#revocation), [Shutdown](#shutdown), [Backup and restore](#backup-and-restore), [Headers](#response-headers), [Limits and proxies](#request-limits-and-reverse-proxies), [Signed releases](#signed-agent-releases), [Troubleshooting by error code](#troubleshooting-by-error-code).

## Installing the server

The supported server deployment is the Docker image on a **Linux host with host networking**, because layer-2 discovery (ARP, MAC addresses) needs to see the LAN.

### One-command installer

**Proxmox VE**, using the [community-scripts](https://github.com/community-scripts/ProxmoxVE) engine:

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/doomedramen/hope/main/ct/hope.sh)"
```

`ct/hope.sh` and `install/hope-install.sh` use the community-scripts engine for container creation, resource selection, downloads and the in-container `update` command. `json/hope.json` carries the website metadata. Nothing is compiled in the container: the server asset is `hope-server-x86_64-linux.tar.gz`, accompanied by `hope-agents.tar.gz`. Updates select one release, download to a separate staging directory, and use the package's `tools/activate-server.py` to back up and activate the server. The engine's installed-version marker changes only after activation succeeds. The old source checkout, toolchains and signed agent release are retained. Agent publication follows successful server activation and uses the same selected release and signing key.

`.github/workflows/server-build.yml` publishes server and agent packages after CI succeeds for a push to `main`; tagged releases use `release.yml`. The container installation creates `/etc/hope/release-signing.pem` and signs the agents into `/var/lib/hope/releases` with the package's `tools/assemble-release.sh`. Updates reuse that key. A signing failure keeps the previous signed agent release available.

Inside the container: the package in `/opt/hope` (`hope-server`, `web/`), the binary also at `/usr/local/bin/hope-server`, settings in `/etc/hope/hope.env` (mode 600, `HOPE_WEB_DIR=/opt/hope/web`, `HOPE_DATA_DIR=/var/lib/hope`, `HOPE_RELEASE_DIR=/var/lib/hope/releases`), the database in `/var/lib/hope`, and a `hope.service` running as root (an LXC runs one application and is the isolation boundary, so community scripts create no service accounts) with `ProtectSystem=strict`, `ReadWritePaths=/var/lib/hope` and `PrivateTmp`. Caddy (the Debian package) terminates HTTPS for the container IP with `tls internal`; its root certificate is copied to `/etc/hope/ca.crt` and named in `HOPE_AGENT_CA`, so generated agent commands expect that file at the same path on every agent host. The first-run setup token is `HOPE_SETUP_TOKEN` in `/etc/hope/hope.env` (a fixed random value, so it survives restarts until the first account exists; the server otherwise generates a new one on every start). The engine's own settings apply as usual (`var_cpu`, `var_ram`, `var_disk`, `var_brg`, `var_net`, `var_unprivileged` and friends; `COMMUNITY_SCRIPTS_URL` overrides the scripts root to test a fork or branch). Defaults: 1 core, 512 MB RAM, 4 GB disk, unprivileged Debian 13; the server itself typically uses well under 200 MB. A container created with the earlier 4 GB default can be shrunk after its first update, on the Proxmox host: `pct set <id> -memory 512 -cores 1`. Verified: `bash -n`, shellcheck with the engine's own policy (warnings such as a bare `cd` under `catch_errors` are not errors), seventeen structural tests in `tests/installer/test_community_scripts.py`, and a check that every helper the pair calls is defined in `community-scripts/core`. Not yet verified: a complete run on a real Proxmox host, and the first release published by `server-build.yml`.

**Any other systemd Linux host**:

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/doomedramen/hope/main/scripts/install-hope.sh)"
```

`scripts/install-hope.sh` is interactive (whiptail when present, plain prompts otherwise) and idempotent: re-running upgrades the binary and console, re-signs the agent release and keeps `/etc/hope/hope.env`, the signing key and the database. On a Proxmox host it simply hands over to `ct/hope.sh` above.

- It installs packages (`curl`, `ca-certificates`, `tar`, `openssl`, Python 3) on Debian/Ubuntu, Fedora/RHEL or Arch families, creates the no-login account `hope`, places the binary in `/opt/hope/bin` and the console in `/opt/hope/web`, writes `/etc/hope/hope.env` (mode 640) and a hardened `hope.service` (`User=hope`, `AmbientCapabilities=CAP_NET_RAW` and nothing else, `ProtectSystem=strict`, `ReadWritePaths=/var/lib/hope`), grants discovery capability through systemd, enables the unit and prints the setup token from the journal.
- **Reverse proxy.** By default it installs the distribution's Caddy package and writes `/etc/caddy/Caddyfile` (an existing file is kept as `Caddyfile.before-hope`). A public host name gets a public certificate; `localhost`, `.local`, `.lan`, `.home`, `.internal`, `.home.arpa` names and bare IP addresses use `tls internal`, and the installer then exports Caddy's root certificate to `/etc/hope/ca.crt` and sets `HOPE_AGENT_CA=/etc/hope/ca.crt`, so generated agent commands expect that file at the same path on every agent host (copy it there). Choose "I run my own reverse proxy" to skip Caddy; the server listens on `127.0.0.1:8080`.
- **Binary source.** The latest GitHub release asset `hope-server-x86_64-linux.tar.gz` (verified against `SHA256SUMS-server`) when one exists; otherwise it offers a source build (rustup and Node.js under `/opt/hope/build`, `git clone`, `npm run build`, `cargo build --release`; 10 to 20 minutes). Release packages are produced by `scripts/package-server.sh` in `server-build.yml` and `release.yml`; each holds `hope-server`, `web/`, `VERSION`, `tools/assemble-release.sh` and `tools/activate-server.py`. Packages predating the activation helper are refused before replacing installed files; use a current package with this installer.
- **Agent release.** With a release install, the installer also downloads the release's `hope-agents.tar.gz`, verifies it against the same `SHA256SUMS-server` (a mismatch aborts before anything is installed) and signs it into the configured `HOPE_RELEASE_DIR` (default `/opt/hope/releases`) after server activation succeeds with the package's `tools/assemble-release.sh --version "$(cat VERSION)"`, so the console's Add agent dialog works right after the install. The key is this server's own `/etc/hope/release-signing.pem` (RSA 3072, created with `umask 077` on the first install, root only); re-runs re-sign with the same key and never replace it, because installed agents trust its public key, so back it up with `/etc/hope/hope.env`. The release is root-owned and world-readable (directories 755, files 644), which is what the `hope` account needs. If signing fails (for example agents and server of different versions, or an older version than the published one), the installer warns, keeps the previous release live and finishes. A release without `hope-agents.tar.gz`, a source build, or a `/opt/hope/releases` already published by hand with a key kept elsewhere is left as it is and reported in the closing summary; publish by hand as described in [Signed agent releases](#signed-agent-releases).

### Server activation and recovery

The host installer and Proxmox updater share `scripts/activate-server.py` (Python standard library only). An exclusive lock prevents concurrent activation. The helper stops `hope.service`, confirms that its process has exited, and makes a consistent SQLite backup including committed WAL data. It also saves the prior executable, console, environment, unit, version and packaged tools before replacing any installed files. The backup and journal are root-private in `/var/lib/hope-activation`.

Startup must pass both systemd's running-state check and `/healthz`, whose process ID must match the unit's `MainPID`. Failure restores the files and pre-migration database, including ownership and permissions, and restarts a previously running installation. A previously inactive service stays inactive after failure. A failed first activation removes its new server files and database. Configuration and agent signing are staged so a failed server upgrade does not publish new agents. Caddy configuration is handled separately by the host installer.

The journal and a systemd startup guard make interruption recoverable. Normal termination attempts rollback. After SIGKILL or power loss, the startup guard refuses an unattended start while an unfinished activation exists. Run recovery as root, then retry the installer:

```sh
python3 /var/lib/hope-activation/recover.py recover
```

Both upgrade entry points attempt this recovery before starting another activation. If recovery itself fails, the command exits unsuccessfully and retains the journal and backups; inspect the reported error rather than deleting the activation directory or starting the server against a partially restored database. Python 3 must remain installed for the startup guard.

The latest completed attempt retains its previous files and database under `/var/lib/hope-activation/previous`; the next successful activation replaces that backup. Allow free space for a complete database and console/tool backup. Recovery restores pre-upgrade state, so observations accepted during an activation that subsequently fails are not retained. Relocating `HOPE_DATA_DIR` is a separate maintenance operation; an upgrade cannot change it at the same time.

Fault-injection tests exercise real SQLite and file operations with stubbed systemd/HTTP, including migration rollback, WAL backup, failed startup/health, concurrent activations, SIGTERM, SIGKILL and retry. These are not evidence of a completed Linux/systemd or Proxmox acceptance run.

Every prompt has an environment answer for unattended use; unset values prompt (or take the default with `HOPE_INSTALL_NONINTERACTIVE=1`):

| Variable | Default | Purpose |
| --- | --- | --- |
| `HOPE_INSTALL_TARGET` | `lxc` on Proxmox, else `host` | `host` installs on this machine; `lxc` hands over to the community-scripts engine (Proxmox only) |
| `HOPE_INSTALL_URL` | `https://<primary IP>` | Canonical HTTPS URL written to `HOPE_SERVER_URL` (never overwritten on re-runs) |
| `HOPE_INSTALL_PROXY` | `caddy` | `caddy` installs Caddy; `none` leaves the proxy to you |
| `HOPE_INSTALL_TLS` | `auto` | `internal` or `public` forces Caddy's certificate source |
| `HOPE_INSTALL_SOURCE` | `auto` | `release`, `source`, or `auto` (release when published, else source after confirmation) |
| `HOPE_INSTALL_VERSION` | latest | Release tag to install, for example `v0.1.0` |
| `HOPE_INSTALL_REF` | `main` | Git ref for source builds |
| `HOPE_INSTALL_REPO` | `doomedramen/hope` | Repository for releases and source |
| `HOPE_INSTALL_NONINTERACTIVE` | unset | `1` never prompts; defaults apply |
| `HOPE_INSTALL_PLAIN` | unset | `1` uses plain prompts even when whiptail exists |
| `HOPE_INSTALL_CT_SCRIPT_URL` | `ct/hope.sh` of `HOPE_INSTALL_REPO` on `main` | The community-scripts `ct` script a Proxmox host hands over to |
| `HOPE_INSTALL_NODE_VERSION` | `22.22.0` | Node.js used by source builds |
| `HOPE_INSTALL_SCRIPT_URL` | raw GitHub URL of the script | Where the container fetches the installer from |
| `HOPE_INSTALL_RELEASE_URL`, `HOPE_INSTALL_ROOT` | unset | Test knobs: a local base URL holding the package, and a prefix for every absolute path (`tests/installer/test_server_installer.py`) |

The host installer has been exercised through its stub harness (download, checksum, layout, unit, env file, Caddy configuration, re-run, and the Proxmox hand-over) and shellcheck, not yet on a real Debian VM. See [validation.md](validation.md).

```sh
cp .env.example .env            # set HOPE_SERVER_URL to your public https:// origin
docker compose pull             # ghcr.io/doomedramen/hope-server:latest (HOPE_IMAGE selects another tag)
docker compose up -d            # compose.yaml: network_mode: host, cap_drop ALL + NET_RAW, read-only root
docker compose logs hope | grep 'setup token'
```

**Published images.** `.github/workflows/docker.yml` builds the Dockerfile's `runtime` stage for Linux x86_64 and publishes it to the GitHub Container Registry as `ghcr.io/doomedramen/hope-server`. Pushes to `main` publish `main` and `sha-<commit>`; a `v*` tag publishes `<version>`, `<major>.<minor>`, `latest` and `sha-<commit>`. Each image carries a build-provenance attestation (`gh attestation verify oci://ghcr.io/doomedramen/hope-server:<tag> --owner doomedramen`) and an SBOM. `compose.yaml` defaults to `latest`, which exists from the first version tag; set `HOPE_IMAGE=ghcr.io/doomedramen/hope-server:main` to follow the branch, or run `docker compose up -d --build` to build from the checkout. The image is not called `ghcr.io/doomedramen/hope` because that package already exists and belongs to another repository; a workflow's `GITHUB_TOKEN` can only push to packages linked to its own repository, so the first run failed with `denied: permission_denied: write_package`. To use that name instead, add this repository with the Write role under the package's "Manage Actions access" settings (or delete the old package) and change `IMAGE` in `docker.yml`. The `hope-server` package is created by the first successful push; if it is private, make it public under the package settings. Not yet verified: a successful run of the workflow.

`compose.desktop.yaml` is for Docker Desktop (macOS/Windows): it publishes `127.0.0.1:8080` and runs `HOPE_DISCOVERY_MODE=limited`, which reports reduced capabilities (no ARP/MAC/physical topology, probes only reach what the container network routes). Use it to try the console, not for discovery of a real LAN.

Without Docker: `make build` (type generation, static web export, `cargo build --release --locked --workspace`), then run `target/release/hope-server` from the repository root (it serves `web/out`). The binary needs `CAP_NET_RAW` for ICMP and ARP (on Linux `setcap cap_net_raw=ep`; the image does this), otherwise TCP probing remains and the console shows the limitation.

| Variable | Default | Purpose |
| --- | --- | --- |
| `HOPE_SERVER_URL` | `http://localhost:8080` | Canonical origin (scheme and authority only, no path/query/credentials). `https` is required except for `localhost`/`127.0.0.1`/`[::1]` development |
| `HOPE_BIND` | `127.0.0.1:8080` (image: `0.0.0.0:8080`) | Listen address. `compose.yaml` and `.env.example` default it to `127.0.0.1:8080`, overriding the image's `0.0.0.0`, so a same-host proxy is trusted automatically in the documented host-network setup; `compose.desktop.yaml` keeps the image's `0.0.0.0` (published only on host loopback), so there nothing is trusted unless `HOPE_TRUSTED_PROXIES` is set; any other bind also needs it |
| `HOPE_DATA_DIR` | `data` (image: `/var/lib/hope`) | Holds `hope.db` |
| `HOPE_WEB_DIR` | `web/out` (image: `/opt/hope/web`) | Static console export served by the Rust server |
| `HOPE_RELEASE_DIR` | `releases` (image: `/opt/hope/releases`) | Signed agent releases, see [Signed agent releases](#signed-agent-releases) |
| `HOPE_SETUP_TOKEN` | random, printed at start | First-run token (at least 24 characters) |
| `HOPE_DISCOVERY_MODE` | `native` on Linux, `limited` elsewhere | `limited` disables ARP |
| `HOPE_AGENT_CA` | empty | Path **on agent hosts** of a private CA, embedded in install commands (see below) |
| `HOPE_REQUIRE_PERSISTENT_SERVICE` | unset | Installer: `1` refuses (`LINGER_REQUIRED`, before any change) a Linux user service when systemd lingering is off for the account; set by SSH jobs |
| `HOPE_RESET_CREDENTIAL` | unset | Agent/installer: `1` makes `enroll`/`configure` replace the credential (requires an enrollment token; the console sets it for revoked agents) |
| `HOPE_SIGNING_KEY` | none | `assemble-release.sh` / `package-agent.sh`: RSA private key outside the repository |
| `HOPE_STAGING_DIR` | `dist/agents-dev` | `package-agent.sh` staging directory; must be under `./dist` or the temp directory |
| `HOPE_TRUSTED_PROXIES` | empty | Reverse proxies whose `X-Forwarded-For` is believed |
| `RUST_LOG` | `hope_server=info` | Log filter |

Capacity: up to 250 active agent identities and one administrator. Revoking or uninstalling an agent releases its slot; its identity and device history remain available, and an identity-bound reset can reactivate it when capacity is available.

## Health checks

`GET /healthz` is a process liveness check and stays successful while the HTTP process can answer. `GET /readyz` returns 200 only when the server is not shutting down and SQLite answers `SELECT 1` within one second; it returns 503 with the failed check otherwise. The Docker image uses `/healthz` so a database fault does not automatically restart the process. Use `/readyz` when a proxy or orchestrator should stop sending new work to a server that cannot reach its database.

The authenticated `GET /api/v1/system/status` endpoint reports the last successful cycle and last error age for discovery, retention, notification evaluation and notification delivery. A task becomes stale after 15 minutes, 5 minutes, 3 minutes and 30 seconds respectively; it is `starting` until its first successful cycle. The response also reports notification outbox depth and oldest age, plus the oldest raw host metric sample and time beyond its 24-hour rollup boundary. Notification backlog older than 5 minutes or metric retention lag over 1 hour marks the overall status `degraded`. A successful delivery loop does not mean the remote notification service accepted every message; use the outbox age for that distinction. Source-metric retention lag is not included yet.

## Canonical URL and reverse proxy

The server does not terminate TLS. Put it behind an existing HTTPS reverse proxy and set `HOPE_SERVER_URL` to the exact public origin users and agents will use, for example `https://hope.example.com`. This single value is used for:

- the origin check on state-changing requests (a mismatching `Origin` is rejected with 403),
- the `Secure` cookie flag and HSTS (only when the scheme is `https`),
- the install commands shown in the console and sent over SSH (the installer refuses `http://`, `INSECURE_URL`).

Proxy requirements: forward `Host` and `Origin` unchanged, pass `X-Forwarded-For`, do not buffer or time out the `/api/v1/events` server-sent-event stream (the console also polls every 15 s as a fallback), and do not override the response headers. Set `HOPE_TRUSTED_PROXIES` to the proxy's address as the server sees it (see [Request limits](#request-limits-and-reverse-proxies)); with the default loopback bind and a same-host proxy this is automatic.

A minimal Caddy site, `hope.example.com { reverse_proxy 127.0.0.1:8080 }`, is the intended shape. Caddy 2.10.2 with exactly that site shape (TLS from a private CA, no flush or header tuning) was tested on 2026-10-15: server-sent events arrive within 0.1 s, the `Secure` cookie, HSTS and `no-store` pass through, a foreign `Origin` is refused, and with `HOPE_TRUSTED_PROXIES` set to the proxy's address the rate-limit key is the real client (a spoofed `X-Forwarded-For` is overwritten by Caddy and ignored); without it all clients share one key. **nginx and Traefik have not been tested.** See [validation.md](validation.md#release-gate-fixture--2026-10-15).

## First-run setup

While no administrator exists, every start prints `H.O.P.E first-run setup token: <token>` to standard error (`docker compose logs hope`). A new random token is generated on each start unless `HOPE_SETUP_TOKEN` is set; only its SHA-256 is stored. The console asks for this token plus a password of 8 to 256 characters (hashed with Argon2). Failed attempts are rate limited per client. Setup can run only once.

**Changing the password.** Open Account at the bottom of the sidebar (the person icon in the header on narrow screens, or "Change password" in the command palette): enter the current password and the new one twice (8 to 256 characters, different from the current one). `POST /api/v1/account/password` (`PasswordChange`) verifies the current password with the same Argon2 check and per-client rate limit as sign-in (a wrong one is refused with 403 and counts as a failed attempt), stores the new hash, ends every session and issues a fresh session cookie to the browser that made the change, so other signed-in browsers must sign in again.

**A lost password** cannot be recovered from the console, but it can be reset without losing data. Stop the server, back up `hope.db`, delete the stored hash and the sessions, and start it again; the server is then back in first-run setup and prints (or takes from `HOPE_SETUP_TOKEN`) a setup token, while hosts, history and enrollments are kept:

```sh
systemctl stop hope            # docker compose stop hope
cp /var/lib/hope/hope.db /var/lib/hope/hope.db.before-reset
sqlite3 /var/lib/hope/hope.db "DELETE FROM meta WHERE key='password_hash'; DELETE FROM sessions;"
systemctl start hope           # then open the console and complete setup with the token
```

Sessions last 24 hours (`hope_session` cookie, `HttpOnly`, `SameSite=Strict`).

## Custom CA: server, agent and SSH destination

Agents and the installer always verify the server's certificate; there is no insecure mode. If the proxy certificate is issued by a private CA, the CA is needed in different places for different parties:

| Party | Needs the CA? | Where |
| --- | --- | --- |
| `hope-server` | No. It serves plain HTTP behind the proxy and has no TLS client | None |
| Reverse proxy | Holds its own certificate and key | Proxy configuration |
| Agent hosts (manual install) | Yes | A PEM file at the same path on every host, set as `HOPE_AGENT_CA` on the **server**; the generated command passes it as `curl --cacert PATH` and `HOPE_CA_CERT=PATH`. The agent reads the file once at enrollment and stores the PEM in its own configuration |
| SSH destination | Yes, the same path | The server does **not** copy the file. If it is missing the job fails with `CA_UNREADABLE` |
| Browsers | Normal trust store | Not H.O.P.E specific |

The bundle must contain valid PEM certificates and be at most 64 KiB. Replacing the CA on an enrolled agent means re-running the installer (`upgrade`) or `hope-agent configure` with `HOPE_CA_CERT` set; the installer's configure step passes it through, but this replacement was not separately validated on a real host.

## Discovery: initial scans and monitoring

Adding a range (IPv4 CIDR, `/20` or smaller: a shorter prefix such as `/16` is rejected, so add several `/20`-or-smaller ranges instead) starts a scan. A range's `enabled` flag controls **monitoring**: the background loop (`discovery.rs`) checks every minute, starting 20 s after the server starts, and begins a new scan of each enabled range whose interval has passed since its last scan. The interval is per range, every 5 minutes by default; Networks in the console (or `PUT /api/v1/networks/{id}` with `interval_minutes`) offers 5, 15 or 30 minutes, 1 or 6 hours, or once a day. Disabled ranges are scanned only when you press scan. A range with an active scan is never queued twice.

**Scheduled scans are gentle.** A scan you start by hand runs at full speed (below). A scheduled scan probes at most four addresses at once and starts a new address at most every 60 ms: about 80 probe operations per second at peak, so a `/24` takes about 15 seconds and a `/20` under four minutes. The expensive part of discovery, the common-port scan, runs only for new devices and then once a day per device, not on every pass; a scheduled pass otherwise re-checks only liveness (ICMP, ports 22/80/443 and ARP).

Removing a range (Networks in the sidebar, or `DELETE /api/v1/networks/{id}`) cancels its active scan, deletes its scan history and drops the membership from hosts that were in it; the hosts themselves, their agents and their history stay.

All ranges share one scheduler: up to 64 concurrent host probes, taken round-robin per range so a silent range does not delay a responsive one, plus an independent bounded enrichment queue (eight workers) for service and name lookup. Scans can be cancelled and show progress. A host that does not answer is **not** declared offline; discovery only records what it observed and when.

Measured completion on the Linux fixtures (two `/24`s, one HTTP responder each, one run per mode in Ticket 09, five in Ticket 03): first responder in 0.77 to 0.82 s, both ranges complete in 25.397 to 25.437 s (second range routed) and 26.392 to 26.464 s (both on-link). Raw data: [discovery-routed.json](benchmarks/discovery-routed.json), [discovery-on-link.json](benchmarks/discovery-on-link.json). These numbers come from OrbStack bridges on one Mac, not a physical LAN; no timing is claimed for real networks.

## Capability limitations

The console shows these as a banner when they apply.

| Limitation | Effect |
| --- | --- |
| Docker Desktop / `HOPE_DISCOVERY_MODE=limited` | No ARP, no MAC addresses, no physical topology; only routable probing from inside the container network |
| macOS server host | `limited` by default (ARP is Linux-only) |
| No `CAP_NET_RAW` or ICMP socket access | No ICMP ping; TCP probing continues (a refused connection still proves a host is alive) |
| ARP applies to on-link subnets only | Routed ranges get ICMP/TCP only; MAC addresses are unknown for them |
| Agents | Report only their own machine, never remote networks. CPU, memory, network and filesystem metrics are collected on the supported Linux x86_64 agent target; GPU utilisation, memory and temperature are unavailable unless a native interface exposes them (not on macOS Apple GPUs: inventory only), and disk I/O counters are unavailable on some platforms. Unavailable values are shown as unavailable, never as zero |
| Map | Links are labelled observed, inferred or manual. Physical switch connections are never invented |
| Active ARP unavailable | Shown as "Active ARP is unavailable: the server could not open a raw packet socket on any network interface…". Devices are still found by ping and TCP, but without MAC addresses there are no manufacturers, most device types stay unknown, and agents cannot be matched to discovered hosts by MAC. See below |
| Platforms | Linux x86_64 server and agents; Windows, other architectures, distributed discovery, switch integrations (SNMP, LLDP, controller APIs) and multiple accounts are out of scope |

### "Active ARP is unavailable"

Active ARP means the server sends its own ARP requests on the local network and reads the replies, which is how it learns each device's MAC address. It needs a raw packet socket (`AF_PACKET`) on an IPv4 Ethernet interface, which requires the `CAP_NET_RAW` capability on Linux. When no interface can be opened, the server falls back to ICMP and TCP and the console shows this message. At startup the server now logs one line per interface (`journalctl -u hope | grep ARP`): `active ARP enabled` for each interface it opened, or the error for each it could not open.

Fixes, by cause:

- **Missing capability** (the log shows `Operation not permitted`). Give the process `CAP_NET_RAW`: the community-scripts container service and the host installer's unit set `AmbientCapabilities=CAP_NET_RAW`; with Docker add `cap_add: [NET_RAW]` (as `compose.yaml` does). On an existing community-scripts install, run its `update` command to install the capability drop-in and restart the service. If the drop-in is rejected, Proxmox is withholding the capability from the container; check the CT's capability policy rather than making it privileged or disabling its AppArmor profile. Do not add `RestrictAddressFamilies` without `AF_PACKET` to the unit.
- **No interface on the scanned LAN.** ARP only works for ranges the server is directly attached to. A routed range (a VLAN reached through a router) never gets ARP; put the server, or a second network interface of its container, on that VLAN.
- **Docker Desktop or `HOPE_DISCOVERY_MODE=limited`.** Containers there cannot see the LAN's layer 2 at all; run the Linux host-network deployment.

## Device types

Every device gets a type (router, switch, access point, hypervisor, NAS, server, virtual machine, computer, phone, tablet, TV, media streamer, smart speaker, printer, camera, robot vacuum, home appliance, game console, smart home device or unknown), a specific label when one is known (for example "Proxmox VE", "Dishwasher", "Chromecast", "Synology NAS"), a confidence and the reasons behind it. `classify.rs` scores every piece of evidence:

- the manufacturer of the MAC address, from the IEEE registry embedded in the server (`crates/hope-server/data/oui.tsv`, regenerated with `scripts/update-oui.py`; see `data/OUI-NOTICE`). Proxmox, QEMU and Docker virtual NICs mark virtual machines, and a private (randomised) MAC, as phones use on Wi-Fi, counts towards a phone;
- open ports (8006 Proxmox VE, 8007 Proxmox Backup Server, 9100/631/515 printing, 554 cameras, 62078 Apple devices, 8009 Google Cast, 8123 Home Assistant, 8291 MikroTik, 3389 Windows and so on);
- names from reverse DNS, mDNS and UPnP (iphone, roborock, dishwasher, diskstation, pve, opnsense, …), the web interface title, mDNS service types and TXT models (for example `_homeconnect._tcp` with a Bosch or Siemens model code identifies the appliance family: SMV… is a dishwasher, WAX… a washing machine), UPnP device types (an internet gateway device is a router);
- being the server's default gateway or a traceroute hop (a router), and what an agent reports.

Short keywords only match whole words ("nas" matches "nas-01", not "jonas-iphone"). A host still named after its address takes the name the device announces. The type can be corrected in host details (Device, `PUT /api/v1/hosts/{id}/type` with a kind, or `null` for automatic detection); a type set by hand is never replaced by a guess. Most of this evidence needs active ARP (manufacturer) or a device on the same link (mDNS, SSDP); routed ranges get ports and web titles only.

## Network hierarchy

Each device gets the device it sits behind (`parent_id`, with the reason), recomputed every minute:

- A range the server is directly attached to sits behind the server's default gateway (from the kernel routing table) when the gateway lies inside the range.
- A routed range is traced; the last hop before the destination is its gateway, and earlier hops chain the routers up to the server. Routers found this way that are not in a monitored range are added as hosts (discovery method `traceroute`).
- An agent's own default gateway is used when nothing else applies.

In Topology and Health mode the map draws each router above the ranges it serves, with one edge per range (and per router hop), instead of a line to every device; a device behind a different router than the rest of its range gets its own edge. Routers keep a saved position of their own. Switches and access points forward frames without appearing in routing tables or traceroutes, so which switch port or access point a device hangs off is not inferred: that needs the switches' own tables (SNMP bridge MIB, LLDP) or a controller API, which H.O.P.E does not integrate. Manual links remain available for those.

## Notifications

Notifications go through [ntfy](https://ntfy.sh), hosted or self-hosted. In the console open Notifications in the sidebar: turn them on, enter the server (default `https://ntfy.sh`), a topic and optionally an access token, choose the events, and use Send test. Subscribe to the same topic in the ntfy app. On a public server anyone who knows a topic can read it, so pick one that is hard to guess, or use a self-hosted server with access control and a token. The token is stored in the server database, is never returned by the API (`ntfy_token_set` says whether one is stored) and is sent as a Bearer token. `GET`/`PUT /api/v1/notifications` and `POST /api/v1/notifications/test` are the API.

Events:

| Setting | Sent when | Priority |
| --- | --- | --- |
| New devices | A device the server has never seen joins a monitored range (not during a range's first scan, which would announce the whole network) | 3 |
| Offline | An agent has not reported for 120 s; or a router, switch, access point, server, NAS or hypervisor without an agent was not seen by a scan that finished after it was last seen, for at least two scan intervals and 15 minutes (phones and other devices that come and go never alert) | 4 |
| Problems | A filesystem or pool 90% full (critical from 97%), a pool not ONLINE, a sensor at 85 °C or more, failed systemd units, failing monitors. CPU and memory spikes are left to the console | 3 or 4 |
| Recoveries | A follow-up when one of the above clears | 2 |

Problems are evaluated every minute into the `alerts` table whether or not notifications are on, so turning them on later never replays problems that were already open. Opening and recovery messages are queued in SQLite with their condition changes. A separate worker delivers up to four independent messages concurrently, with a ten-second request deadline and retries from five seconds up to fifteen minutes apart. Pending messages survive a server restart; a recovery waits until the opening message has been accepted. Delivery is at least once: a crash after ntfy accepts a message but before SQLite records the acknowledgement can produce a duplicate.

The outbox holds at most 4,096 messages. At capacity, condition transitions wait for space and are reconsidered on the next evaluation; a condition that appears and clears during that interval can go unannounced. New-device announcements at capacity are omitted with a server warning. Disabling notifications or changing the server/topic clears pending messages; changing the token wakes retries. Turning off an event category cancels its queued messages when the worker next checks them. An HTTP request already in progress can still finish. Notification delivery runs independently of discovery and condition evaluation, and shutdown cancels in-flight delivery while retaining unacknowledged messages. Send test is immediate and reports its result directly instead of entering the retry queue.

Each message links to the host in the console (`HOPE_SERVER_URL/?host=…`). The last delivery time or error is shown in the dialog.

## Probe traffic

For each address in a scanned range the server may send: one ICMP echo request (payload `HOPE`, 750 ms deadline), TCP connection attempts to ports 22, 80 and 443 (750 ms deadline each; connect-only, no payload), and, for on-link addresses on Linux with ARP enabled, one ARP request (250 ms deadline). A per-host budget of six seconds applies.

A device seen for the first time, and every device once a day after that, gets an enrichment pass (30 s budget):

- **Common ports.** TCP connects (connect-only, no payload, 750 ms each, through the same rate limit) to 40 ports chosen to tell device types apart: 21, 22, 23, 25, 53, 80, 111, 139, 143, 443, 445, 515, 548, 554, 631, 1883, 2049, 3000, 3306, 3389, 5000, 5001, 5432, 5900, 6443, 7000, 8006, 8007, 8008, 8009, 8080, 8123, 8291, 8443, 8883, 9000, 9090, 9100, 32400 and 62078 (the list and the names shown are `COMMON_PORTS` in `fingerprint.rs`). The result replaces the previous result for these ports; ports outside the list that agents or quick probes recorded are kept.
- **Web interface.** If one of the web ports (8006, 443, 80, 8443, 8080, 5001, 5000, 8123, 8007, 9000, 3000) is open, one `GET /` to the first that answers, reading at most 64 KiB to record the page `<title>` and the `Server` header. Certificates are not verified (LAN devices almost always use self-signed ones); no credentials or cookies are sent, and these requests never use a proxy.
- **Reverse DNS**, through the system resolver (`getent hosts` / `dscacheutil`, killed at its deadline, output capped).

Every five minutes the server also listens for what devices announce: one mDNS query (DNS-SD enumeration plus 29 common service types, unicast-response bit set, sent twice) to `224.0.0.251:5353`, and one SSDP `M-SEARCH` (sent twice) to `239.255.255.250:1900`, each collecting answers for three seconds. For each SSDP answer whose `LOCATION` is a plain-HTTP URL on the answering address itself, one `GET` reads the UPnP description (64 KiB at most). Once an hour, ranges the server is not directly attached to are traced: ICMP echo requests with TTL 1 to 8, one per hop, one second each. All responses are parsed with bounded sizes, counts and string lengths. No authentication attempts happen, and no UDP ports are scanned.

Probe starts are limited to 200 per second shared by all ranges, with at most 64 sockets in flight. This limits scheduled probe operations, not frames: TCP handshakes, retransmitted SYNs and kernel neighbour resolution produce extra packets, and **no absolute packet-rate or per-scan frame count has been measured**. Only scan networks you are authorised to probe.

Agent traffic is one HTTPS `POST /api/v1/agent/report` every 10 seconds by default (choose 5, 10, 15, 30 or 60 seconds under Networks → This server; agents adopt it from their next report response) with a Bearer credential; inventory is attached on enrollment, when it changes, and at least hourly. Measured at the former 30-second interval on macOS arm64 over 30 minutes (application bytes, TLS overhead excluded): about 5.2 kB up and 0.64 kB down per report, roughly 0.7 MB per hour per agent at that interval (about 2.1 MB at 10 seconds); agent resident memory 14 MB (physical footprint 5 MB) and 0.15% of one core. Evidence: [agent-macos-arm64-apple-m4-pro.json](benchmarks/agent-macos-arm64-apple-m4-pro.json), summary in [validation.md](validation.md#agent-resource-benchmarks-ticket-11). If the server is unreachable, an agent queues reports on disk (bounded to 720 reports, 4 MiB, one hour) and retries with exponential backoff up to 15 minutes, so reconnection after a long outage can take minutes (measured: 148 s after a 300 s outage).

## Identity reconciliation

A discovered host (observed by address/MAC) and an enrolled agent are joined as follows:

- At enrollment the agent's interface MAC addresses are compared with **unassigned** discovered hosts. Exactly one match: the agent takes over that host record and its history. Zero or several matches: a new host is created; IP address alone is never used because DHCP may have reassigned it.
- Repeated discovery of an ambiguous MAC does not create more duplicates.
- If two records are the same machine, merge them in the console (host details, merge). The merge keeps history and removes the source's map positions in every map mode.
- A revoked identity keeps its host and history. Reinstalling with an administrator-issued, identity-bound token (the console offers this for revoked agents) replaces only its credential; the old credential stays rejected.
- Agent identity (UUID) is created by the agent at first enrollment and kept in its state directory. A host reinstalled with a wiped state directory enrolls as a new identity and must be merged by hand.

## Installing agents

Agents are installed from the console (a command that fetches the server-embedded installer over HTTPS) or through an SSH job started from the console.

Manual command (copy from the console; the token lasts 15 minutes, is single use and is stored only hashed):

```sh
( umask 077; installer=$(mktemp) || exit 1; trap 'rm -f "$installer"' EXIT
  curl --fail --silent --show-error --location --max-time 30 --proto '=https' --proto-redir '=https' \
    https://hope.example.com/agent/install -o "$installer" \
  && HOPE_SERVER=https://hope.example.com HOPE_ENROLLMENT_TOKEN=... sh "$installer" )
```

The installer runs the phases `preflight`, `stage`, `verify`, then a journaled transaction (`stop`, `place-binary`, `configure`, `write-unit`, `activate`, `wait-report`) and `complete`, printing `hope-install: phase=NAME` and `hope-install: error=CODE message` on standard error. Success is declared only after the installed service's own accepted report is seen (exact process and activation identifier), by default within 120 s (`HOPE_FIRST_REPORT_TIMEOUT`). It verifies the signed manifest, the artifact size, SHA-256 and signature, and the binary's version before touching the existing service.

Installer modes (`HOPE_INSTALL_MODE=auto|user|system`, default `auto`): as root, or with passwordless sudo, a system service is installed; otherwise a user service. Mixed existing installs are refused (`EXISTING_INSTALL_MODE`). `HOPE_REQUIRE_PERSISTENT_SERVICE=1` (set by SSH jobs, optional for manual runs) makes a Linux user service conditional on systemd lingering for the account: otherwise the installer fails in preflight with `LINGER_REQUIRED` before changing anything, because a user manager without lingering stops about 10 s after the last session ends. Without the variable (a manual, interactive install) the installer proceeds and prints `hope-install: notice=USER_SERVICE_NOT_PERSISTENT`. Over SSH, an account without sudo therefore needs `sudo loginctl enable-linger USER` first.

| | Linux system | Linux user | macOS system | macOS user |
| --- | --- | --- | --- | --- |
| Binary | `/usr/local/bin/hope-agent` | `~/.local/bin/hope-agent` | `/usr/local/bin/hope-agent` | `~/.local/bin/hope-agent` |
| State and credential | `/var/lib/hope-agent` | `~/.local/state/hope-agent` | `/var/lib/hope-agent` | `~/.local/state/hope-agent` |
| Service definition | `/etc/systemd/system/hope-agent.service` | `~/.config/systemd/user/hope-agent.service` | `/Library/LaunchDaemons/io.hope.agent.plist` | `~/Library/LaunchAgents/io.hope.agent.plist` |

Linux system services run as the dedicated no-login account `hope-agent`; the systemd unit adds `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`, `PrivateTmp` and an empty capability set. Linux user services stop at logout unless an administrator runs `loginctl enable-linger USER`. A noexec `TMPDIR` is reported as `BINARY_NOT_EXECUTABLE`, not worked around.

On a Linux Proxmox VE host with `pvesh`, the system installer also installs a root-owned, non-overlapping collector timer. It refreshes inventory every 60 seconds and writes resource samples every 30 seconds. The collector reads the cluster VM index to learn current ownership, but reports only the local node's QEMU and LXC guests, including stopped guests. Guest lists and live guest counters are fetched in bulk from node-local API indexes; guest configuration is read from allowlisted pmxcfs config files and cached for five minutes. Slow enrichment rotates through at most one QEMU guest-agent query and two live LXC interface queries per cycle, under short timeouts. QEMU guest-agent enrichment is read-only and only attempted when enabled and responding; guest OS and filesystem data carry their own observation times, and unavailable filesystem values are omitted. No guest commands are executed. Hypervisor allocation/configuration (memory, disks, NIC bridge/VLAN) is kept separate from optional guest-reported OS filesystem use.

The cluster UUID, random enrollment token, guest incarnations and ownership generations are stored under `/etc/pve/priv/hope-agent`, protected by Proxmox's distributed lock mechanism. The token authorizes other enrolled agents in the same shared source scope and is not included in inventory devices or returned by server APIs. The collector imports old per-node incarnation state and sends metadata-only aliases for that reporter's own previous records; the server verifies those aliases against the authenticated reporter before reconciliation. Shared state is kept when an agent is uninstalled. A quorum loss makes pmxcfs read-only, so collection may become incomplete until the cluster filesystem is writable again. These paths and API behavior are unit/fixture tested; this repository has not been live-validated on Proxmox VE 8 or 9.

SSH jobs: the console first reads the host key fingerprint **without authenticating**; you must confirm it. The job is bound to the confirmed key (a different key aborts before any credential is sent, `HOST_KEY_MISMATCH`), runs the same command through `sh -s`, uses only non-interactive `sudo -n`, and parses only the known progress grammar. Passwords and keys are not stored. Default limits: connect 15 s, inactivity 5 minutes, total 20 minutes, first report 2 minutes; output beyond 1 MiB ends the job. Outcomes are `completed`, `failed` (meaning known), `uncertain` (the remote state could not be determined: inspect the host) or `interrupted` (server restarted). History keeps 100 jobs or 30 days, with at most four running at once.

**Verification status of installation:** the installer is tested with stubbed service managers (104 harness tests, 27 of them for the updater below) and, before the updater existed (2026-10-15), on real Debian 13 hosts with systemd 257 (arm64 native, amd64 emulated): system service as root and as a sudo user, user service with lingering, custom CA, reinstall, interrupted activation (rolled back), upgrade, downgrade refusal, uninstall and purge, and SSH jobs against a real OpenSSH 10.0. **No launchd (macOS) cell, no default-CA run and no other distribution has been run.** Matrix and limits: [validation.md](validation.md#release-gate-fixture--2026-10-15).

## What agents report

Live state uses a durable report sequence within each enrollment identity. The agent reserves each number in `report-sequence.json` before queueing its sample; the server commits its sequence watermark with the displayed host data in migration 17's `agent_report_heads` table. Clock corrections do not change this order. Old queued reports may fill history but cannot replace a newer live sample. The watermark survives history retention and server restart.

If an agent is restored from an old backup or loses its counter, it learns the server watermark from an accepted response and reserves higher numbers for newly collected samples. Queued history is never renumbered. Service activation is confirmed only after a current-process sample becomes live; acknowledging old history alone does not confirm installation. Preserve the counter with the enrollment identity during backup and ordinary uninstall; identity purge removes both. Invalid or unwritable counter state stops reporting with an explicit error rather than reusing numbers.

`last_collected_at` is the agent's observed wall time. `metrics_received_at` is the server receipt time of the displayed sample, while `last_report` records receipt of accepted nonduplicate reports, including older queued samples. Host details show both measurement times. The attention list warns when measurements stop advancing for two minutes despite arriving reports, or the sample timestamp differs from receipt by at least one minute. A timestamp behind receipt can mean clock skew or queued data. Resource notifications neither open nor resolve a problem from stale measurements. Valid reports must still fall within the existing timestamp window: at most five minutes ahead or 24 hours behind the server.

Upgrade the server before agents. Legacy reports continue to use timestamp ordering until the identity first sends a sequenced report; after that, unsequenced reports contribute history only. Rolling that identity back to an older agent without sequence support will not advance live measurements: install a version supporting report ordering to resume them. New agents still communicate with older servers, but those servers retain their timestamp-ordering limitation.

At each report interval (with jitter) an agent sends CPU, memory (used, total, cache and buffers from `/proc/meminfo`, ZFS ARC size from `/proc/spl/kstat/zfs/arcstats`, swap), load averages, network throughput and per-interface counters, filesystem usage, host-wide and per-device disk throughput (`/proc/diskstats`, the root device flagged), ZFS pools (`zpool list -Hp` plus per-dataset read and write counters under `/proc/spl/kstat/zfs/<pool>`), temperature sensors (hwmon on Linux, SMC on macOS), GPUs, systemd service units (every failed unit, plus the watched list, with active/failed/total counts from `systemctl list-units`) and the results of configured network monitors. Each collector reports its availability; a source that is missing on a platform yields "unavailable", never an estimate. Rates need a previous sample, so the first report after a start has none.

Agents also send observed system uptime and boot time using sysinfo, plus Linux's boot UUID when readable. Host details label these as values at the last sample; they do not keep ticking during an outage. Containers show **Kernel uptime** and a scope explanation because their visible counters and boot ID may describe the host kernel. Older agents have unavailable uptime. Historical aggregates retain the latest boot observation, without averaging across reboots. This supplies boot metadata, not a reboot reason or a reboot notification.

Network counters and inventory share a deterministic selection of at most 128 non-loopback interfaces, prioritizing the default-route interface and then physical interfaces under the existing name-based fallback policy. Each interface has at most 64 distinct, sorted addresses; IPv6 link-local addresses are excluded because the protocol has no interface-zone field. The `network_inventory` collector marks truncation explicitly. Addresses and counters come from the same refreshed sysinfo snapshot.

Two settings are read at agent start, from environment variables on the service (`systemctl edit hope-agent` adds a drop-in) or from `settings.json` in the agent state directory (`{"monitors": [...], "watch_services": [...]}`; the variables win when set):

| Variable | Purpose |
| --- | --- |
| `HOPE_MONITORS` | Comma-separated reachability checks run every sample with a five-second deadline each: `tcp://host:port` (connect latency) and `http://` or `https://` URLs (GET, status below 400 counts as up; redirects are not followed; the agent's custom CA is trusted). At most 32 |
| `HOPE_WATCH_SERVICES` | Comma-separated systemd service names (with or without `.service`), trimmed and deduplicated before the 64-name limit. Observed watched entries take priority within the 128-unit detail limit, including `not-found` entries. Currently limited to the loaded-unit snapshot; see below |

The console graphs memory as a stack (used excluding ARC, ARC, cache and buffers) under a total line, load averages, per-device and per-pool throughput, pool allocation, temperatures (the hottest eight sensors, filterable) and monitor latency, and raises attention items for degraded or nearly full pools, sensors at 85 °C and above, failed units and monitors that are down.

Service summaries retain the full observed failed-unit count even when the detail list is bounded or omitted. The console explains missing detail instead of reporting no failures, and notifications use that summary count. An unloaded watched service is still a coverage gap: `list-units --all` is not an inventory of every installed definition. Explicit unit queries and expected-running policies remain planned; absence from the current snapshot must not be taken as proof that a service is missing or healthy. [systemctl documentation](https://www.freedesktop.org/software/systemd/man/latest/systemctl.html).

## Agent upgrade, rollback and uninstall

In the console, open a host's **Settings** tab and choose **Uninstall agent → Uninstall over SSH**. Inspect and verify the host fingerprint, then supply the installation owner's SSH credentials (root or passwordless sudo for a system service). The job stops and removes the service, updater and binary while keeping the enrollment identity and server history. It requires the remote uninstall phase, completion marker and successful SSH exit before reporting success, then revokes the agent credential and returns the host to the discovered-device fleet. The host leaves the Agents list, while historical metric rows remain stored and the identity stays available for re-enrollment. It does not issue an enrollment token or wait for a report from the removed agent. Connection loss or incomplete output leaves the outcome uncertain; inspect the device before retrying. Install and uninstall jobs share concurrency limits and cannot run together on the same host or SSH address. Credentials are discarded after authentication.

SSH uninstall sets `HOPE_EXPECT_AGENT_ID` so the uninstaller checks the selected identity against the device's configuration before changing any service. This works with older agents and refuses a different identity or missing configuration. Custom CA paths must exist on the device. For a custom state directory, use **Uninstall manually** and set `HOPE_AGENT_DIR` before running the command. The manual tab also lets you disable access before copying the command; after confirming removal on the device, choose **I removed the agent** to return its host to the discovered-device fleet. The uninstall script is available even when no signed agent release is published. If no service manager is available or the service cannot be stopped, uninstall fails and keeps the installation files. Linux system uninstall also takes the updater's installation lock, so it cannot race an active update.

Fetch the installer from the server (as above) and run it with an operation argument:

```sh
HOPE_SERVER=https://hope.example.com sh installer upgrade      # existing identity is kept; no token needed
sh installer uninstall                                          # service and binary removed, identity kept
HOPE_PURGE_IDENTITY=1 sh installer uninstall                    # also deletes the enrolled identity and spool
```

`upgrade` requires an existing installation (`NOT_INSTALLED` otherwise). It refuses a release older than the installed agent (`DOWNGRADE_REFUSED`; override with `HOPE_ALLOW_DOWNGRADE=1` on the host). Everything from `stop` onward is journaled: on any failure, signal or leftover transaction from an interrupted run, the previous binary, service definition and settings are restored and the previous service is restarted (`ROLLED_BACK`; if restoration itself fails, `ROLLBACK_INCOMPLETE` and the backup is kept for the next run). The enrolled credential is never discarded by a rollback. Signals are handled only after the current foreground command (for example a slow download) returns. If nobody reads the installer's output any more (a closed pipe, a dropped SSH session) it fails at its next progress line and rolls back like any other failure, so the previous service is restored; it may also have finished before that line, which is why SSH jobs report `uncertain` rather than guessing.

### Updating agents from the console

A system-service agent (Linux or macOS, installed as root or with sudo) can be updated with one click: **Update now** on the host, or **Update N agents** on the overview. The console shows what can be done for each agent that runs an older release than the server offers: *ready* (it has an updater), *pending* (it was asked and starts when it next reports) or *needs the upgrade command once* (see below).

How it works, and why it is safe to trigger from the network:

1. The administrator's request is stored on the host and returned as `update_to` in the agent's next report response. It lapses after a day if the agent never acts on it.
2. The agent itself cannot replace its own binary: it runs as an unprivileged account with a read-only system. It only creates `update-queue/request` in its state directory (at most once an hour per release if the update keeps failing). On Linux, systemd retries transient download or busy-lock failures five minutes apart, with at most twelve starts per hour. It also watches the root-owned install transaction so an interrupted update can be recovered without another agent report.
3. A service-manager unit that runs as root (a systemd path unit `hope-agent-update.path`, or a launchd daemon `io.hope.agent.update` watching the queue directory) then starts `/usr/local/sbin/hope-agent-update`. On Linux it first acquires a kernel-owned `flock` lock (from util-linux); manual installations share that lock, and a busy updater retains the request. That script then deletes the request without reading it and runs the installer in `upgrade` mode with `HOPE_ONLY_NEWER=1`, so the binary is replaced only when the signed manifest offers a newer release than the one installed.
4. The installer it runs is the copy kept in `/var/lib/hope-agent-update/` (`Library/Application Support/hope-agent-update/` on macOS), root-owned, with **the verification key this host was installed with embedded**. The key is not fetched at update time, so a server that starts publishing under another key, or someone who intercepts the connection, cannot make the host accept it (`MANIFEST_INVALID`). All the usual checks apply: manifest signature, platform entry, size, SHA-256, the binary's own signature, the version the binary reports, the downgrade refusal, a journaled transaction that rolls back on any failure, and waiting for the new agent's first accepted report.
   The updater also refuses to run, and reports `STATE_UNSAFE`, if it finds any symbolic link in the agent's state directory: the agent never creates one, and this runs as root over files the agent account can write.
5. The outcome is written to `update-result` and reported by the agent as the `agent_update` collector (`Updated to 0.1.60`, `Checked: already on 0.1.60`, `The last update failed (CODE)`). On failure the previous version keeps running.

What this does not change, and what it widens: the signing key still decides what hosts will run, and the server decides when to ask. Before this feature, a compromised server could not change a host that already had an agent unless an administrator ran the installer there. Now a compromised server that **holds the signing key** could sign a malicious release and ask every updatable agent to take it. The Proxmox script and `install-hope.sh` keep that key on the server itself. If that exposure matters, sign releases on another machine with `scripts/assemble-release.sh` or `release.yml` ([Signed agent releases](#signed-agent-releases)) and copy only the result to the server: the server then cannot produce a release the hosts accept, and the updater still refuses anything else. A request always comes from a signed-in administrator's click or a server that could equally have been asked to publish; nothing starts on a schedule.

Where the updater is installed, a host has: `/usr/local/sbin/hope-agent-update` (on Linux, run `sudo systemctl start hope-agent-update.service` to retry manually and update systemd's status; running the script directly also works but leaves the service's previous status unchanged), `/var/lib/hope-agent-update/{install-agent.sh,server,ca}`, and the two units. Only a **full installation** (the install command with a new token) replaces the pinned installer and key, which is also how you move hosts to a new key after [rotation](#rotation-and-revocation). Uninstalling the agent removes the updater.

Installer settings behind this: `HOPE_ONLY_NEWER=1` makes `upgrade` change nothing unless the server's release is newer than the installed agent (the upgrade command and the updater both set it); `HOPE_UPDATER=1` is set by `hope-agent-update` and tells the installer to leave the installed updater and its pinned key as they are. `HOPE_BUILD_VERSION` is set by the build workflows for published builds (`<major>.<minor>.<run number>`): it is compiled into the agents and written to the server package's `VERSION`, which the server signs the agents with; leave it unset for local builds, which use the crate version.

Agents installed before this feature, and user-service installs (no root, so nothing can run on their behalf), have no updater. The host's update card then shows the token-less **upgrade command**, which is `upgrade` mode with `HOPE_ONLY_NEWER=1` and no secret in it. Run it once on a system-service host and it installs the updater; from then on the console can do it. It also works as a manual update for user services, every time.

Verification status: the updater is tested with stubbed service managers (`tests/installer`: pinned key, rejected key change, tampered binary, rollback with a result, links left in the state directory, uninstall; shipped agent releases target Linux x86_64), and the request and result hand-off with the agent and server tests. On 2026-10-16 it also ran for real: `tests/gates/run.sh updater` (also the `updater-gate` CI job) boots Debian 13 with systemd 257 in a container and passed all six cells there (install, update all through the path unit, a no-op request, a release signed by another key, a signed release whose agent cannot report being rolled back, uninstall). That run found and fixed a real bug: a rollback restarts the agent, which forgot it had just asked and retried the failing release in a loop; the agent now records its last request in `update-asked`. **Not yet run: launchd (macOS), other distributions, or the CI job on GitHub's runners**; run one update on macOS before relying on it there.

Running the raw uninstall script does not notify the server. Revoke access in the console too, or confirm removal in the manual uninstall flow. `HOPE_PURGE_IDENTITY=1` deletes only known agent files; created service accounts are kept. Reinstalling after a purge creates a new identity.

Server upgrade: back up (below), pull or rebuild the image, `docker compose up -d`. Migrations run forward on open; a database from a newer server is refused rather than modified. There is no downgrade migration: roll the server back by restoring the pre-upgrade backup with the older image.

Agent release rollback (publishing an older signed release) is described under [Publishing](#publishing).

Linux system installations require `flock` from util-linux. To repair an older installed updater, run the console's upgrade command on the host once: it now refreshes the pinned installer, wrapper and units even when the agent binary is current. Automatic binary updates continue to preserve the installed trust key and updater implementation; publishing a new binary alone does not migrate old updater scripts. This also migrates older saved updaters to immutable release downloads. Publishing a binary alone leaves their previous download behavior in place. The process-lock, transaction-watcher and immutable-download changes have harness coverage; re-run the real Linux updater gate before a production release.

## Revocation

In the console, open the host's **Settings** tab and choose **Disable agent**. The agent's credential hash is marked revoked in the same transaction as the host flag; a report already queued behind the write lock cannot commit after revocation. Reports are blocked immediately, including for offline devices. At its next server contact, the updated agent receives the explicit `agent_revoked` error, saves `disabled: true` in its configuration, clears queued telemetry and exits with status 0. systemd `Restart=on-failure` and launchd's unsuccessful-exit policy do not restart it. A later service start or device reboot exits before collecting or sending anything; the service and binary remain installed until uninstalled. History stays.

Generic HTTP 401/403 denials are retried because a reverse proxy can produce them transiently. Upgrade the server before deploying this agent behavior; an older server's generic revocation response will be retried rather than stopping the agent. Older agents may exit without remembering the disabled state, or keep retrying; uninstall removes their service as well. To reconnect a disabled agent, issue the identity-bound reset token from the console and reinstall. Only successful credential replacement clears the local disabled state; an upgrade, ordinary configure, or rollback does not. Signing-key compromise is a separate event, see [Rotation and revocation](#rotation-and-revocation).

## Shutdown

`hope-server` handles SIGTERM (Docker, systemd) and SIGINT (Ctrl-C) identically. On either signal it:

1. stops accepting scan and SSH-job requests and stops the periodic monitoring and retention timers;
2. cancels in-flight scans and SSH installation sessions, and ends open event streams (browsers reconnect on their own);
3. waits for connections to drain, then persists unfinished work: scans become `interrupted`, SSH jobs become `interrupted` with code `SERVER_RESTARTED` (the remote host may be partly installed; inspect it before retrying);
4. checkpoints and truncates the SQLite write-ahead log and closes the database.

The internal deadline is 8 seconds from the signal, inside the compose stop grace periods (`compose.yaml`: 15 s; `compose.desktop.yaml`: 10 s, also Docker's default). If connections do not close in time the server logs a warning and exits anyway; unfinished rows are still marked interrupted. A hard kill (SIGKILL, power loss) skips this; the next start marks any `queued`/`running` scans and jobs as interrupted, so no state is left claiming to run.

## Backup and restore

The database is `$HOPE_DATA_DIR/hope.db` (`/var/lib/hope/hope.db` in the image). It runs in WAL mode, so committed data may live in `hope.db-wal` rather than the main file. **Never copy `hope.db` alone while the server runs.**

Create a consistent backup of the live database (SQLite `VACUUM INTO`, one snapshot, safe during writes):

```sh
docker compose exec hope hope-server backup /var/lib/hope/backup-$(date +%Y%m%d-%H%M%S).db
# or, outside Docker:
HOPE_DATA_DIR=/path/to/data hope-server backup /path/to/backup.db
```

The command refuses to overwrite an existing file, creates the copy with mode 0600 (it holds password, session and credential hashes), checks it with `PRAGMA integrity_check`, and produces a standalone file with no `-wal`/`-shm` sidecars. Move backups off the host (`docker compose cp hope:/var/lib/hope/backup-….db .`) and treat them as secrets.

What is not in the database: agent release artifacts and signatures (`./releases`), the optional custom CA file referenced by `HOPE_AGENT_CA`, and environment configuration (`.env`). Back those up separately.

Restore:

1. Stop the server (`docker compose stop hope`) so nothing holds the database.
2. Replace `hope.db` with the backup and **delete stale `hope.db-wal` and `hope.db-shm`**. A leftover WAL from the previous database would be replayed against the restored file and corrupt it.
3. Ensure the file is owned by the service user (uid 10001 in the image).
4. Start the server. Migrations run on open; a backup from a newer server version is refused rather than modified.

In Docker the data lives in the `hope-data` volume (its real name is prefixed with the compose project name; find it with `docker volume ls`). With the server stopped:

```sh
docker volume ls   # the volume is named <compose-project>_hope-data, e.g. hope_hope-data
docker run --rm -v hope_hope-data:/data -v "$PWD":/backup debian:bookworm-slim \
  sh -c 'rm -f /data/hope.db-wal /data/hope.db-shm && cp /backup/backup.db /data/hope.db && chown 10001:10001 /data/hope.db'
```

**What a restore does to agents.** The database holds agent credential hashes and revocation flags as of the backup, and an agent that receives a 401 or 403 on a report concludes it is revoked, exits with status 0 and stays down. Therefore:

- Agents enrolled **after** the backup are unknown to the restored server; their reports are rejected and they stop. Re-enroll each one with a new enrollment token (run the installer again with `HOPE_RESET_CREDENTIAL=1`, which makes the agent replace its credential and enroll again under the same identity). This path is implemented in the agent but has not been rehearsed after a real restore.
- Credentials revoked **after** the backup are valid again. Re-revoke them in the console immediately after restoring.
- Reports newer than the backup are absent from history; spooled reports on agents that survive are delivered.

Take backups often (for example daily, and before every upgrade). Restore checklist: stop the server; restore the file and delete the WAL/SHM; start; check the console; re-revoke credentials revoked since the backup; list agents that stopped reporting and re-enroll those enrolled since; confirm `/healthz` and a fresh report from one agent.

## Response headers

Every response carries `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`, a restrictive `Content-Security-Policy` (`frame-ancestors 'none'`, `object-src 'none'`, `base-uri 'none'`, everything else `'self'`) and a deny-all `Permissions-Policy`. `Strict-Transport-Security: max-age=31536000` is sent only when `HOPE_SERVER_URL` is `https://`.

Every `/api/` response (authentication, enrollment, install commands, SSH jobs, event stream, errors) also carries `Cache-Control: no-store` and `Pragma: no-cache`. Static console files are left cacheable.

The CSP allows inline scripts (`script-src 'self' 'unsafe-inline'`). The Next.js static export embeds inline bootstrap scripts whose contents differ per page and build, so they cannot be pinned by a fixed hash without a post-build hashing step. Styles allow inline for React Flow and chart style attributes. All other sources, including `connect-src`, are `'self'`. Do not weaken or override these at the reverse proxy.

Session cookies are `HttpOnly; SameSite=Strict`, plus `Secure` when `HOPE_SERVER_URL` is `https://`. State-changing requests (anything but GET/HEAD/OPTIONS) are rejected when `Origin` differs from the canonical URL or `Sec-Fetch-Site` is anything other than `same-origin`/`none`; agents and `curl` send neither header.

## Request limits and reverse proxies

Limits count **failures per client** and never use a shared (global) budget, because any shared ceiling can be filled by an attacker from many addresses to lock out the administrator or legitimate agents. Successful logins, enrollments and accepted reports consume nothing from the failure budget; a successful login clears the client's failures.

| Endpoint | Per-client limit | Notes |
| --- | --- | --- |
| Login and first-run setup | 10 failures/minute, then 429 | No global ceiling. Password hashing concurrency is capped at 2; a flood gets a transient 503 with `Retry-After: 2` (not counted as a failure), never a lockout |
| Agent enrollment | 20 failed requests/minute | Successful enrollments do not count |
| Agent reports with bad credentials | 30 rejections/minute | Checked before the global write lock is taken |
| Authenticated reports | 120/minute per agent identity | Keyed by the authenticated identity only; no global ceiling |

The key table holds 4096 clients. When full it drops expired entries, then the least recently active one; keys are never pooled, so a new client always starts with its own budget. IPv6 clients are keyed by /64 and IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) are treated as IPv4.

"Client" is the TCP peer. Behind a reverse proxy every request arrives from the proxy, so the proxy must be trusted to supply `X-Forwarded-For` (the right-most entry that is not itself a trusted proxy is used; left-hand entries are client supplied and ignored):

- `HOPE_TRUSTED_PROXIES` (comma-separated addresses/CIDRs) sets the list explicitly.
- When it is unset and `HOPE_BIND` is loopback-only (the default, and the documented same-host proxy setup), loopback (`127.0.0.0/8`, `::1`) is trusted automatically: only local processes can reach such a listener.
- For any other bind (the Docker image and `compose.desktop.yaml` bind `0.0.0.0`; `compose.yaml` defaults to loopback) nothing is trusted by default; set the variable to the proxy address as the server sees it.

The server logs a warning once if an untrusted peer sends `X-Forwarded-For`, and once if a trusted proxy sends an unparseable hop. Without a correct trusted list all clients share one key, so ten failures from anyone would block everyone for a minute.

Rejected request bodies (malformed JSON, wrong content type, oversize) receive fixed messages that never quote the submitted data.

Container capability note: the `hope-server` binary has a file capability (`cap_net_raw`), so the container's bounding set must contain `NET_RAW` (compose `cap_add`); without it the binary fails to execute. Do not enable `no-new-privileges`. Run backups with `docker compose exec hope hope-server backup /var/lib/hope/<name>.db`: the root filesystem is read-only, so the destination must be on the data volume or another writable mount, and the file is fsynced (with its directory) before the command reports success.

## Signed agent releases

`$HOPE_RELEASE_DIR` (`./releases` in compose) is a root directory written only by `scripts/assemble-release.sh`. It holds versioned release directories `<version>-<random>/` and the symlink `current` pointing at the live one (plus `previous`). New installers and upgrade attempts select `current` once, so publishing takes effect without a restart while downloads already in progress keep their selected release. Installation and availability require this managed directory layout. Legacy unversioned download URLs remain readable, including flat hand-built trees, but flat trees cannot offer new installations; publish them with the assembler first. Each release directory contains:

```
hope-agent-linux-x86_64                     supported agent binary
hope-agent-….sig                            per-artifact RSA/SHA-256 signature
manifest.json, manifest.json.sig            signed version / key id / sha256 / size / minimum OS per artifact
release-public.pem                          the public key embedded into /agent/install
```

A freshly fetched `/agent/install` embeds the selected release ID and its public key together. Initial installation downloads only from `/agent/releases/by-id/<id>/…`. Each upgrade selects an ID once from the uncached public `/agent/releases/current-id` endpoint, then uses that immutable directory for every manifest, signature and binary request. A saved updater retains its installed trust key across these selections. Publishing between requests cannot mix releases. Keep downloaded installers for no longer than 24 hours after their release is replaced; fetch a fresh installer if its release has expired.

The installer downloads `manifest.json`, verifies its signature against the key embedded in the installer it was served by, picks its platform entry, then checks the artifact's size, SHA-256 and own signature, and finally that `--version` equals the manifest version. Any mismatch aborts before the service is touched (`MANIFEST_INVALID`, `CHECKSUM_MISMATCH`, `BAD_SIGNATURE`, `RELEASE_UNAVAILABLE`). An upgrade to an older version than the installed agent is refused (`DOWNGRADE_REFUSED`) unless the host runs the installer with `HOPE_ALLOW_DOWNGRADE=1`. The server lists a platform as available only if the manifest is well formed, its `key_id` equals the SHA-256 of the SPKI DER of `release-public.pem`, it lists the platform, and the artifact and signature exist with the listed size. With no manifest nothing is available (no per-file fallback); the console and SSH jobs then report that no signed release manifest is published. The downgrade check compares only with the agent installed on that host: a fresh install, or a host whose agent is already old, can receive any older release the server's key signed, so publish only intended releases (rollback is a deliberate act, see below).

### Platforms

| Platform | Target | Minimum |
| --- | --- | --- |
| Linux x86_64 | `x86_64-unknown-linux-musl` | Static binary, no glibc; any distribution. Kernel 3.2 or later |


Support is not claimed for any platform until the installation matrix in `docs/validation.md` is evidenced for it.

### The signing key

One RSA key (at least 2048 bits; use 3072), generated and stored **outside the repository and outside the container image**:

```sh
umask 077
openssl genrsa -out ~/secure/hope-release-signing.pem 3072
```

`assemble-release.sh` refuses a key path inside the repository. A server installed with `scripts/install-hope.sh` has its own key, `/etc/hope/release-signing.pem`, created by the installer and used for every re-run (see [One-command installer](#one-command-installer)); to publish by hand on such a server, use that key (`HOPE_SIGNING_KEY=/etc/hope/release-signing.pem … --out /opt/hope/releases`), or the publish is refused as a key change. Keep an offline backup; losing the key means every host must be re-installed from a new installer. In CI the key is the `HOPE_SIGNING_KEY` secret of the protected `release` GitHub environment, available only to the `publish` job of `release.yml` (tag pushes `v*`); pull-request workflows reference no secrets. Configure required reviewers and restrict the environment to `v*` tags.

### Publishing

1. Set the workspace version in `Cargo.toml`, merge, tag `vX.Y.Z` (the tag must equal the crate version). `release.yml` builds the supported Linux x86_64 target, uploads it, and the `publish` job assembles, signs and verifies it.
2. To publish to a server by hand: download the `agent-linux-x86_64` artifact into one directory and run
   `HOPE_SIGNING_KEY=~/secure/hope-release-signing.pem scripts/assemble-release.sh --staging DIR --version X.Y.Z --out releases`.
   It checks the artifact version and git commit and verifies its ELF header matches Linux x86_64, builds `releases/X.Y.Z-<random>/`, verifies every signature, checksum, size and the key id there, then atomically repoints `releases/current` (a temporary symlink renamed over it with `mv -T`/`mv -h`). A failure leaves `current` untouched. Every replaced release is retained for at least 24 hours after replacement, including during rapid consecutive publications. The live release, the two newest older releases and the previous release are kept beyond that window; later publications prune other expired directories. Version must be plain `X.Y.Z` or `X.Y.Z-pre` (no leading `v`).
3. Mount the **root** (`./releases:/opt/hope/releases:ro`, as the compose files do), never a versioned directory: the root keeps its identity, and the relative `current` symlink resolves inside the container, so no restart is needed after publishing.
4. Roll back by repointing: `ln -s <older-dir> releases/.c && mv -fh releases/.c releases/current` (BSD/macOS; use `mv -fT` on GNU). Hosts that already upgraded keep the newer agent; re-running the installer on them will refuse to downgrade unless `HOPE_ALLOW_DOWNGRADE=1`. Re-publishing with `--allow-downgrade` also works.

Local development: `HOPE_SIGNING_KEY=… scripts/package-agent.sh` builds and publishes the supported Linux x86_64 target into `./releases`. Its staging directory must be under `./dist` or the temp directory. To change development key, delete `./releases` first; `--rotate-key` requires a complete release. The Linux x86_64 target can also be built in Docker with `scripts/build-agent-docker.sh x86_64-unknown-linux-musl --out DIR`.

### Rotation and revocation

Publishing a different public key next to old artifacts would break verification for some hosts, so the script refuses it. To rotate: `assemble-release.sh … --rotate-key` with the new key and the supported Linux x86_64 artifact (it cannot be combined with `--allow-partial`); every artifact and the manifest are signed with the new key in that run and the old release stays available as `previous`. Initial installers already fetched keep installing their original release within its retention window. Upgrade attempts select the new release, but a saved installer with the old trust key rejects it with `MANIFEST_INVALID`. Hosts must fetch `/agent/install` again from the server to accept the new key; hosts with an updater keep rejecting it until a manual installation replaces the key it pinned ([Updating agents from the console](#updating-agents-from-the-console)). Rotation does not revoke agent credentials; revoke those in the console. A compromised signing key requires rotating, restarting the server, and re-installing from freshly fetched installers; there is no automatic revocation of binaries already installed.

## Troubleshooting by error code

Every installer failure ends with `hope-install: error=CODE message` on the host's standard error. SSH jobs show the same codes with vetted text from a server-side catalog (remote text is never displayed or stored), and the job outcome is `failed` when the meaning is known and `uncertain` when the server cannot know what state the host is in; for `uncertain` codes always inspect the host before retrying. The table lists every code of `scripts/install-agent.sh` and of the SSH catalog in `crates/hope-server/src/ssh.rs`; `tests/docs/test_operations.py` fails if a code is added to either without a row here.

Origin `installer` means the script on the host emits it (so it also appears in manual installs); `server` means only SSH jobs produce it.

| Code | Origin | SSH job outcome | Meaning | Action |
| --- | --- | --- | --- | --- |
| `UNSUPPORTED_PLATFORM` | installer | failed | The host operating system is not supported. | Agents are currently built for Linux x86_64. |
| `UNSUPPORTED_ARCH` | installer | failed | The host CPU architecture is not supported. | Agents are currently built for Linux x86_64. |
| `NO_SERVICE_MANAGER` | installer | failed | No usable service manager was found on the host. | Use a supported Linux x86_64 host with running systemd. For a user service, log in normally so a user manager exists, then retry. |
| `LOCK_UNAVAILABLE` | installer | failed | Linux system installation needs flock from util-linux. | Install util-linux on the host and retry. |
| `INSTALL_BUSY` | installer | failed | Another installation or update owns the agent update lock. | Wait for it to finish, then retry. |
| `SUDO_UNAVAILABLE` | installer | failed | A system installation needs root or passwordless sudo, which is unavailable. | Connect as root, allow passwordless sudo for this account, or retry as an ordinary user to get a user service. |
| `LINGER_REQUIRED` | installer | failed | Nothing was installed: without administrator access this would be a user service, which stops shortly after the SSH session ends because lingering is off for this account. | Ask an administrator to run `sudo loginctl enable-linger <user>` for this account and retry, or retry with an account that has root or passwordless sudo. |
| `EXISTING_INSTALL_MODE` | installer | failed | A conflicting system or user installation already exists on the host. | Remove or complete the other installation on the host, then retry. |
| `NOT_INSTALLED` | installer | failed | No existing installation was found to operate on. | Run a fresh installation instead. |
| `INVALID_MODE` | installer | failed | The requested installation mode is not allowed for this account. | Retry as a user that can create the intended kind of service. |
| `INSECURE_URL` | installer | failed | The server URL is not HTTPS, which the installer requires. | Ask the administrator to configure an https:// canonical server URL, then retry. |
| `MALFORMED_URL` | installer | failed | The server URL is not in a form the installer accepts. | Ask the administrator to correct the canonical server URL, then retry. |
| `CA_UNREADABLE` | installer | failed | The custom CA certificate is missing or unreadable on the host. | Place the CA certificate at the configured agent CA path on the remote host, make it readable by this account, then retry. |
| `PATH_UNWRITABLE` | installer | failed | The installer cannot write to a required location on the host. | Check permissions and symbolic links for the agent directories, or retry with a privileged account. |
| `INVALID_SETTING` | installer | failed | An installer setting was rejected. | Retry; if it persists, ask the administrator to check the installer configuration. |
| `USAGE` | installer | failed | The installer rejected its invocation. | Ask the administrator to check that the server and installer versions match. |
| `RELEASE_UNAVAILABLE` | installer | failed | The server has no signed release manifest listing an agent for this host's platform. | Ask the administrator to publish a signed release (scripts/assemble-release.sh, or scripts/package-agent.sh for development) that includes this operating system and architecture, then retry. |
| `DOWNLOAD_FAILED` | installer | failed | The host could not download from the H.O.P.E server. | Check that the host can reach the server URL over HTTPS and trusts its certificate (or custom CA), then retry. |
| `BAD_SIGNATURE` | installer | failed | The downloaded agent failed signature verification and was not run. | Do not retry until an administrator confirms the release signing key and packages. |
| `MANIFEST_INVALID` | installer | failed | The signed release manifest failed verification or does not match the downloaded agent, so nothing was run. | Do not retry until an administrator re-publishes the release with scripts/assemble-release.sh and confirms the signing key. |
| `CHECKSUM_MISMATCH` | installer | failed | The downloaded agent does not match the checksum or size in the signed manifest and was not run. | Retry once in case of a damaged download; if it repeats, ask the administrator to re-publish the release. |
| `DOWNGRADE_REFUSED` | installer | failed | The server's release is older than the agent already installed on the host. | Publish a newer release, or run the installer with HOPE_ALLOW_DOWNGRADE=1 on the host if a downgrade is intended. |
| `BINARY_INVALID` | installer | failed | The installed agent did not run on the host. | Check that the packaged release matches the host's platform and libc, then retry. |
| `BINARY_NOT_EXECUTABLE` | installer | failed | The host cannot execute programs from its temporary directory. | Remount the temporary directory without noexec, or give this account an executable TMPDIR, then retry. |
| `EXISTING_IDENTITY_MISMATCH` | installer | failed | The host already runs an agent with a different identity. | Reinstall from the host record that owns that agent, or uninstall the existing agent first. |
| `UNINSTALL_STOP_FAILED` | installer | uncertain | The agent service is still loaded or running; installation files were kept. | Stop the service on the device, then retry uninstalling. |
| `UNINSTALL_REVOKE_FAILED` | server | uncertain | Remote uninstall finished, but the server could not revoke the credential. | Disable agent access in host settings and inspect the device before retrying. |
| `SERVICE_ACCOUNT_INVALID` | installer | failed | A pre-existing service account does not meet the installer's safety requirements. | Remove or repair the agent service account on the host, then retry. |
| `CONFIGURE_FAILED` | installer | failed | The agent could not be configured or enrolled. | Check that the host reaches the server over HTTPS; the enrollment token lasts 15 minutes and is single use, so start a new job. |
| `SERVICE_START_FAILED` | installer | failed | The agent service did not start on the host. | Inspect the service manager's logs on the host, then retry. |
| `FIRST_REPORT_TIMEOUT` | installer | failed | The agent started but the server did not receive its first report in time. | Check that the host can reach the server URL, trusts its certificate or custom CA, and that its clock is correct, then retry. |
| `INTERRUPTED` | installer | uncertain | The installer was interrupted on the host. | Inspect the host to see which installation is active before retrying. |
| `ROLLED_BACK` | installer | failed | The installer failed and reported restoring the previous installation. | Check the host before retrying; the previous service should be active again. |
| `ROLLBACK_INCOMPLETE` | installer | uncertain | The installer could not fully restore the previous installation. | Log in to the host and inspect or repair the agent service manually before retrying. |
| `HOST_KEY_MISMATCH` | server | failed | The host key differs from the fingerprint you confirmed. No credentials were sent. | Inspect the host fingerprint again and verify it through a trusted channel. A changed key can mean a reinstall or an interception. |
| `CONNECT_FAILED` | server | failed | The server could not establish an SSH session with the host. | Check the address, port and network path, then retry. |
| `AUTH_FAILED` | server | failed | The host rejected the credentials. Nothing was installed. | Check the username and password or key, then start a new job. |
| `KEY_INVALID` | server | failed | The private key could not be read or decrypted. No connection was made. | Check the key format and passphrase, then start a new job. |
| `CONNECTION_LOST` | server | uncertain | The SSH connection ended before the installer reported a result. Remote state is unknown. | Inspect the host to see whether the agent was installed or the previous installation was kept, then retry. |
| `INACTIVITY_TIMEOUT` | server | uncertain | The installer produced no output for too long. Remote state is unknown. | Inspect the host, which may still be running the installer, before retrying. |
| `TOTAL_TIMEOUT` | server | uncertain | The installation exceeded its time limit. Remote state is unknown. | Inspect the host, which may still be running the installer, before retrying. |
| `OUTPUT_LIMIT` | server | uncertain | The host sent more output than the server accepts, so the session was closed. Remote state is unknown. | Inspect the host before retrying. |
| `INSTALLER_FAILED` | server | uncertain | The installer exited with an error but gave no recognized reason. Remote state is unknown. | Inspect the host, or run the manual installation command there to see the full output. |
| `INSTALLER_INCOMPLETE` | server | uncertain | The installer ended without confirming completion. Remote state is unknown. | Inspect the host, or run the manual installation command there to see the full output. |
| `IDENTITY_MISMATCH` | server | failed | The host enrolled with a different identity than the one this job was bound to. | Check which agent runs on the host and reconcile the host records before retrying. |
| `TOKEN_FAILED` | server | failed | The server could not prepare an enrollment token. | Retry; check the server logs if it persists. |
| `INTERNAL_ERROR` | server | uncertain | The server hit an internal error while running this job. Remote state is unknown. | Inspect the host before retrying, and check the server logs. |
| `HOST_ASSOCIATION_UNCONFIRMED` | server | uncertain | An agent enrolled and reported, but it could not be matched to the selected host. The installation may have succeeded. | Check the host list for the new agent and merge it with the selected host if it is the same machine. |
| `SERVER_RESTARTED` | server | uncertain | The server stopped or restarted while this installation was running. The remote host's state is unknown: it may be partly installed, and its agent may still enroll later. Inspect the host before retrying. | Inspect the host, then start a new installation job. |

### Other symptoms

| Symptom | Likely cause | Action |
| --- | --- | --- |
| Console says too many attempts (429) for everyone | The server sees all clients as the proxy's address | Set `HOPE_TRUSTED_PROXIES` to the proxy address as the server sees it |
| Install command shows `http://` or the installer fails with `INSECURE_URL` | `HOPE_SERVER_URL` is not `https://` | Set the public https origin and restart |
| Agent never appears | Token older than 15 minutes or already used; certificate not trusted by the host | Create a new token; check `curl https://SERVER/healthz` from the host; see [Custom CA](#custom-ca-server-agent-and-ssh-destination) |
| Agent running but host shows no recent report | Reports rejected, or the server clock is far from the agent clock | On the host run `HOPE_AGENT_DIR=STATE hope-agent status --json`; system services log to journald (`journalctl -u hope-agent`), macOS services to `STATE/agent.log` |
| Console shows reduced discovery | `HOPE_DISCOVERY_MODE=limited` or no `NET_RAW` | Use the Linux host-network deployment, see [Capability limitations](#capability-limitations) |
| Hosts listed twice after enrollment | Several unassigned discovered hosts shared a MAC, or none matched | Merge them, see [Identity reconciliation](#identity-reconciliation) |
| `exec: operation not permitted` when starting the container | `NET_RAW` dropped (the binary carries a file capability) | Keep `cap_add: [NET_RAW]` and do not set `no-new-privileges` |
