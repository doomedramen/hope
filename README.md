# H.O.P.E

Home network discovery, host monitoring and a lightweight reporting agent for up to 250 hosts, with a single administrator and one self-hosted server. A Rust server (Axum, Tokio, SQLx/SQLite) scans configured IPv4 ranges, enriches what it finds, and serves a static Next.js console (host list, topology / subnet / health maps, per-host history). An optional Rust agent for Linux and macOS reports a machine's own CPU, memory, network, storage and GPU data to the server over HTTPS every 30 seconds.

**Status: pre-release.** The code builds and passes its automated suites, but the release is **not** complete: key environment-dependent gates are unverified (below). Do not rely on it for anything that matters, and read [docs/validation.md](docs/validation.md) before trusting any claim made here.

## What is and is not in scope

In scope: discovery of IPv4 ranges (`/20` or smaller) with fair concurrent scanning, cancellation and progress; bounded enrichment; map modes (topology, subnets, health) with provenance for observed, inferred and manual links (physical switch connections are never invented); agents for Linux and macOS (x86_64 and ARM64, systemd and launchd); manual and SSH-assisted agent installation through one signed installer; history (raw 24 hours, five-minute aggregates through day seven); revocable per-agent credentials.

Not in scope and not planned for this release: Windows agents, distributed discovery, switch integrations, notifications, multiple user accounts. Agents report only their own machine, never remote networks, and an unanswered probe does not prove a host is offline.

## Release gates: five product stages

| # | Stage | Evidence today | Still open |
| --- | --- | --- | --- |
| 1 | Discovery and hosts | Controlled two-range Docker fixtures (OrbStack, Linux ARM64): [routed](docs/benchmarks/discovery-routed.json) 25.4 s and [on-link](docs/benchmarks/discovery-on-link.json) 26.4 s for both ranges; fairness, cancellation and enrichment tests | A physical LAN with host networking and ARP; Docker Engine on a physical Linux host; Docker Desktop on Windows |
| 2 | Map modes | Playwright acceptance (10 Chromium tests) for topology/subnet/health modes, drag persistence, selection and filters; screenshots reviewed | Tests use synthetic inventory, not data from real discovery |
| 3 | Agent and history | Agent fault tests, 250-host retention load test, a 30-minute [Linux x86_64 LXC measurement](docs/benchmarks/agent-linux-x86_64-lxc.json), and a 30-minute macOS arm64 measurement (see "Resource use") | Linux ARM64 and macOS x86_64 measurements; GPU-collector comparisons; the signed musl artifact was not re-measured |
| 4 | Manual installation | Transactional installer with rollback, covered by a 71-test harness using stubbed service managers and real signatures | **No real systemd or launchd installation, upgrade, rollback or uninstall has been run with the signed artifacts, on any platform** |
| 5 | SSH and release hardening | SSH jobs against an in-process SSH server (host-key binding, progress grammar, timeouts); four-target signed release build and assembly with a throwaway key; CI definitions | A real `sshd`; the CI workflows have never run on GitHub; no production signing key exists |

Details, commands, dates and hashes: the "Release gate status" table in [docs/validation.md](docs/validation.md).

## Supported platforms

Only what is evidenced is claimed. "Supported" in the first release will mean the installation matrix in `validation.md` is green for that cell; **it is empty today**.

| Component | Platform | Evidence |
| --- | --- | --- |
| Server | Docker image on Linux ARM64 (OrbStack VM), host networking and Desktop-mode compose files | Built, healthy, discovery fixtures run |
| Server | macOS arm64 native (development) | Tests and manual use; discovery is limited (no ARP) |
| Server | Linux x86_64, physical Linux host networking, Windows | **Unverified** |
| Agent | macOS arm64 | Built, unit tests, 30-minute resource measurement (this repository's `scripts/bench-agent.sh`) |
| Agent | Linux x86_64 (Debian 13 LXC) | Hand-installed service, 30-minute measurement; not installed through the production installer |
| Agent | Linux ARM64 | Compiles, tests pass in a container, `--version` runs; no service run |
| Agent | macOS x86_64 | Compiles; **never executed** |
| Agent installation | any OS, system or user mode | **Unverified** (see stage 4) |

## Quick start (Docker, Linux host networking)

```sh
# from a checkout of this repository
cp .env.example .env              # set HOPE_SERVER_URL to your public https:// origin
docker compose up -d --build      # compose.yaml: host networking, NET_RAW only, read-only root
docker compose logs hope | grep 'setup token'
```

Put an HTTPS reverse proxy in front of `HOPE_BIND` (default `127.0.0.1:8080`), open the canonical URL, and finish setup with the printed token and a password. Agents and the installer require HTTPS; a private CA is supported ([operations](docs/operations.md#custom-ca-server-agent-and-ssh-destination)). No real reverse proxy (Caddy, nginx, Traefik) has been tested yet.

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
python3 -I -m unittest discover -s tests/docs
npm --prefix web run typecheck && npm --prefix web run lint && npm --prefix web run test:e2e
```

## Resource use of the agent

Targets: below 30 MB resident memory and below 0.5% of one core on average during normal reporting. Results are published only with their evidence file; reproduce with `scripts/bench-agent.sh` (60 s warmup, 1800 s measurement, a server outage phase). Results for the machines measured so far are in [docs/benchmarks/](docs/benchmarks/) and summarised in [docs/validation.md](docs/validation.md#agent-resource-benchmarks-ticket-11).

## Documentation

- [docs/architecture.md](docs/architecture.md): components, data flow, identity reconciliation, security model.
- [docs/operations.md](docs/operations.md): installation, reverse proxy, first-run setup, custom CA, discovery behaviour and probe traffic, agent install/upgrade/rollback/uninstall, revocation, backup/restore, release signing, troubleshooting by error code.
- [docs/validation.md](docs/validation.md): commands, environments, dates, hashes and results; the release gate table; every unverified item.
- [docs/IMPLEMENTATION_HANDOFF.md](docs/IMPLEMENTATION_HANDOFF.md): scope decisions and per-ticket implementation notes.

## License

**Not yet decided.** `Cargo.toml` declares `license = "MIT"` for the workspace, but the repository contains no `LICENSE` file, and a manifest field alone is not a license grant. Until the owner decides and adds a license file, no permission to use, copy, modify or distribute this code is granted by the repository.
