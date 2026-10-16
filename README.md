# H.O.P.E

Home network discovery, host monitoring and a lightweight reporting agent for up to 250 hosts, with a single administrator and one self-hosted server. A Rust server (Axum, Tokio, SQLx/SQLite) scans configured IPv4 ranges, enriches what it finds, and serves a static Next.js console (host list, topology / subnet / health maps, per-host history). An optional Rust agent for Linux and macOS reports a machine's own CPU, load, memory (including cache and ZFS ARC), network, storage, disk and pool throughput, temperatures, GPU, systemd unit and reachability-monitor data to the server over HTTPS every 30 seconds.

**Status: pre-release.** The code builds and passes its automated suites, but the release is **not** complete: key environment-dependent gates are unverified (below). Do not rely on it for anything that matters, and read [docs/validation.md](docs/validation.md) before trusting any claim made here.

## The console

![Overview: fleet health at a glance](docs/screenshots/overview.png)

- **Overview**: hosts responding, agents reporting, what needs attention (stale agents, unobserved hosts, CPU, memory or filesystems above 90%), fleet-wide usage, the busiest hosts and a getting-started checklist until the first agent reports. Every figure comes from the latest sample; nothing is invented for missing data.
- **Hosts**: status filters, sortable columns and inline CPU, memory and disk meters; click a host for details.
- **Network map**: topology, subnet and health modes with provenance for reported, inferred and manual links, a minimap for large fleets, and drag-to-link.
- **Host details**: live usage tiles with sparklines, hardware, storage meters and ZFS pools, per-device throughput, temperature sensors, interfaces and counters, systemd units, network monitors, GPUs, and 1 h / 6 h / 24 h / 7 d history that draws outages as gaps. Open a host's full page for a two-column chart dashboard: CPU, load average, memory (used, ZFS ARC, cache and buffers under the total), bandwidth, disk I/O, pool usage and I/O, temperatures, monitor latency, filesystem and GPU usage.
- **Command palette** (`⌘K` / `Ctrl+K`, or `/`): jump to any host or run any action from the keyboard.
- Light, dark and automatic themes; everything works at phone width.

![Host page: every metric graphed, current values on the charts](docs/screenshots/host-page.png)

| Hosts | Map (health mode) | Host details (dark) |
| --- | --- | --- |
| ![Hosts](docs/screenshots/hosts.png) | ![Map](docs/screenshots/map-health.png) | ![Host details](docs/screenshots/host-details-dark.png) |

Screenshots are taken by the Playwright suite from synthetic inventory (`web/tests/console.spec.ts`).

## What is and is not in scope

In scope: discovery of IPv4 ranges (`/20` or smaller) with fair concurrent scanning, cancellation and progress; bounded enrichment; map modes (topology, subnets, health) with provenance for observed, inferred and manual links (physical switch connections are never invented); agents for Linux and macOS (x86_64 and ARM64, systemd and launchd); manual and SSH-assisted agent installation through one signed installer; history (raw 24 hours, five-minute aggregates through day seven); revocable per-agent credentials.

Not in scope and not planned for this release: Windows agents, distributed discovery, switch integrations, notifications, multiple user accounts. Agents report only their own machine, never remote networks, and an unanswered probe does not prove a host is offline.

## Release gates: five product stages

| # | Stage | Evidence today | Still open |
| --- | --- | --- | --- |
| 1 | Discovery and hosts | Controlled two-range Docker fixtures (OrbStack, Linux ARM64): [routed](docs/benchmarks/discovery-routed.json) 25.4 s and [on-link](docs/benchmarks/discovery-on-link.json) 26.4 s for both ranges; fairness, cancellation and enrichment tests | A physical LAN with host networking and ARP; Docker Engine on a physical Linux host; Docker Desktop on Windows |
| 2 | Map modes and console | Playwright acceptance (17 Chromium tests) for the overview, topology/subnet/health modes, drag persistence, selection, filters, command palette, themes and range removal; screenshots reviewed | Tests use synthetic inventory, not data from real discovery |
| 3 | Agent and history | Agent fault tests, 250-host retention load test, a 30-minute [Linux x86_64 LXC measurement](docs/benchmarks/agent-linux-x86_64-lxc.json), and a 30-minute macOS arm64 measurement (see "Resource use") | Linux ARM64 and macOS x86_64 measurements; GPU-collector comparisons; the signed musl artifact was not re-measured |
| 4 | Manual installation | Transactional installer with rollback, covered by a 77-test harness with stubbed service managers and real signatures, **and a real Linux matrix (2026-10-15)**: Debian 13 with systemd, arm64 native and amd64 emulated, system and user services, custom CA; install, reinstall, interrupted activation, upgrade, downgrade refusal, uninstall, purge | **launchd (macOS) installation, upgrade, rollback and uninstall; the default public-CA path; physical x86_64 hardware and other Linux distributions** |
| 5 | SSH and release hardening | SSH jobs against an in-process SSH server and, since 2026-10-15, against a real OpenSSH 10.0 on Debian 13 (host-key binding, auth failures, user-service fallback, reinstall, dropped connection); a real Caddy 2.10.2 reverse proxy (SSE, Secure cookie, rate-limit key, spoofed `X-Forwarded-For`); four-target signed release build and assembly with a throwaway key; CI definitions | The CI workflows have never run on GitHub; no production signing key exists; nginx and Traefik, `sudo` that needs a password |

Details, commands, dates and hashes: the "Release gate status" table in [docs/validation.md](docs/validation.md).

## Supported platforms

Only what is evidenced is claimed. "Supported" in the first release will mean the installation matrix in `validation.md` is green for that cell; **today only the Linux systemd cells below are green** (the [release gate fixture](tests/gates/README.md), run once on 2026-10-15).

| Component | Platform | Evidence |
| --- | --- | --- |
| Server | Docker image on Linux ARM64 (OrbStack VM), host networking and Desktop-mode compose files | Built, healthy, discovery fixtures run |
| Server | macOS arm64 native (development) | Tests and manual use; discovery is limited (no ARP) |
| Server | Linux x86_64, physical Linux host networking, Windows | **Unverified** |
| Agent | macOS arm64 | Built, unit tests, 30-minute resource measurement (this repository's `scripts/bench-agent.sh`) |
| Agent | Linux x86_64 (Debian 13 LXC) | Hand-installed service, 30-minute measurement |
| Agent | Linux x86_64 (Debian 13, emulated under Rosetta) | Same installer matrix as ARM64, emulated rather than Intel hardware |
| Agent | Linux ARM64 (Debian 13, native container) | Installed through the production installer as a system and a user systemd service; upgrade, rollback, uninstall, SSH job; see the fixture matrix |
| Agent | macOS x86_64 | Compiles; **never executed** |
| Agent installation | Linux, systemd, system and user mode, custom CA | Verified in the fixture matrix (Debian 13 only) |
| Agent installation | macOS (launchd), default public CA, other distributions | **Unverified** (see stage 4) |

## Install in one command

**Proxmox VE** (a Debian LXC container, built by the [community-scripts](https://github.com/community-scripts/ProxmoxVE) engine with its usual default/advanced settings; `update` inside the container rebuilds the current `main` commit, exactly as it does for any community script):

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/doomedramen/hope/main/ct/hope.sh)"
```

**Any other systemd Linux host** (a dedicated account, a hardened service with `CAP_NET_RAW` for discovery, and Caddy as the HTTPS reverse proxy with automatic public certificates or an internal CA for LAN names whose root is exported for agents; re-run to upgrade):

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/doomedramen/hope/main/scripts/install-hope.sh)"
```

The Proxmox pair (`ct/hope.sh`, `install/hope-install.sh`, `json/hope.json`) follows the community-scripts work instructions to the letter so it can be submitted to DevScripts unchanged; the host script uses the latest GitHub release when one is published and otherwise builds from source. Details, environment variables and what has and has not been verified: [docs/operations.md](docs/operations.md#one-command-installer).

## Quick start (Docker, Linux host networking)

```sh
# from a checkout of this repository
cp .env.example .env              # set HOPE_SERVER_URL to your public https:// origin
docker compose up -d --build      # compose.yaml: host networking, NET_RAW only, read-only root
docker compose logs hope | grep 'setup token'
```

Put an HTTPS reverse proxy in front of `HOPE_BIND` (default `127.0.0.1:8080`), open the canonical URL, and finish setup with the printed token and a password. Agents and the installer require HTTPS; a private CA is supported ([operations](docs/operations.md#custom-ca-server-agent-and-ssh-destination)). Caddy 2.10.2 has been tested in front of it (see the gate fixture); nginx and Traefik have not.

**Docker Desktop (macOS, Windows) cannot do layer-2 discovery.** `compose.desktop.yaml` runs in limited mode (no ARP, MAC addresses or physical topology; the console shows this limitation) and binds `127.0.0.1:8080`. Use it to try the console, not to map a LAN.

Agent installation needs a signed release published to `./releases` first ([Signed agent releases](docs/operations.md#signed-agent-releases)); until you publish one, the console shows that no release is available.

## Build from source

Requirements: Rust 1.96 (edition 2024), Node 26 with npm, and for the installer/release tests Python 3 and OpenSSL; ShellCheck is used for shell linting.

```sh
make types                          # regenerate web/lib/protocol.ts from the Rust protocol (never overlap with a web build)
cargo build --release --locked --workspace
npm --prefix web ci && npm --prefix web run build     # static export in web/out
HOPE_SERVER_URL=http://localhost:8080 target/release/hope-server
```

Checks (the full release sequence is in [docs/IMPLEMENTATION_HANDOFF.md](docs/IMPLEMENTATION_HANDOFF.md)):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --locked
make installer-tests release-tests shell-checks
tests/gates/run.sh all      # real systemd/sshd/Caddy gate fixture in disposable Docker containers (tests/gates/README.md)
python3 -I -m unittest discover -s tests/docs
npm --prefix web run typecheck && npm --prefix web run lint && npm --prefix web run test:e2e
```

The browser suite downloads its own Chromium; on a machine that already has one (or cannot download), point `PLAYWRIGHT_CHROMIUM` at the binary.

## Resource use of the agent

Targets: below 30 MB resident memory and below 0.5% of one core on average during normal reporting. Results are published only with their evidence file; reproduce with `scripts/bench-agent.sh` (60 s warmup, 1800 s measurement, a server outage phase). Results for the machines measured so far are in [docs/benchmarks/](docs/benchmarks/) and summarised in [docs/validation.md](docs/validation.md#agent-resource-benchmarks-ticket-11).

## Documentation

- [docs/architecture.md](docs/architecture.md): components, data flow, identity reconciliation, security model.
- [docs/operations.md](docs/operations.md): installation, reverse proxy, first-run setup, custom CA, discovery behaviour and probe traffic, agent install/upgrade/rollback/uninstall, revocation, backup/restore, release signing, troubleshooting by error code.
- [docs/validation.md](docs/validation.md): commands, environments, dates, hashes and results; the release gate table; every unverified item.
- [docs/IMPLEMENTATION_HANDOFF.md](docs/IMPLEMENTATION_HANDOFF.md): scope decisions and per-ticket implementation notes.

## License

MIT. See [LICENSE](LICENSE); the workspace `Cargo.toml` declares the same.
