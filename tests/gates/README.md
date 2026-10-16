# Release gate fixture

Runs the environment-dependent release gates that can be exercised with disposable Docker containers: real systemd and OpenSSH targets, a real Caddy reverse proxy with a throwaway private CA, the real server image built from this tree, and a signed four-target release built with the repository scripts. Nothing is installed on the host, no real host is contacted, and the repository's `releases/` directory is never touched.

```sh
tests/gates/run.sh all      # build, start, run every gate, print the matrix, tear down (about 7 minutes with artifacts staged and caches warm; the first run also builds the images and four agents)
tests/gates/run.sh build    # images, signed release 0.1.0 (and artifacts for 0.1.1)
tests/gates/run.sh up       # network, server, Caddy, administrator
tests/gates/run.sh proxy | matrix | ssh | report | down
tests/gates/run.sh clean-caches   # remove the cargo cache volumes created by build-agent-docker.sh
```

Requirements: Docker (OrbStack on macOS was used), Python 3.9+, OpenSSL, `ssh-keygen`, `rsync`, `git`. macOS artifacts are built natively with `scripts/build-agent.sh`, so the full release (assembly requires all four platforms) needs a macOS host with both Apple Rust targets installed; the launchd service cells cannot run in Docker and are not part of this fixture.

`HOPEGATE_WORK=DIR` selects the working directory (default: a new directory under the system temporary directory). It holds the throwaway release signing key, CA, SSH key, generated passwords, staged artifacts, the temporary releases root and `results.json`. It must be outside the repository (the assembler refuses a key inside it) and is never deleted by the script; remove it yourself.

## What it does

| Part | Detail |
| --- | --- |
| Targets | `target.Dockerfile`: Debian 13 with systemd as PID 1, OpenSSH, sudo and three accounts (`root`, `hopesudo` with passwordless sudo, `hopeplain` without). Built for `linux/arm64` (native) and `linux/amd64` (Rosetta on OrbStack). Run with `--cgroupns=host`, tmpfs `/run`, `SYS_ADMIN`, `seccomp=unconfined` and `/sys/fs/cgroup` mounted; not `--privileged` (that left journald and logind failed). Under Rosetta, systemd units that set `MemoryDenyWriteExecute=yes` crash with SIGTRAP, so the amd64 image switches that option off for the distribution's own units; the agent unit is untouched |
| Server | Image `hopegate-server:gates` from the repository `Dockerfile`, `HOPE_SERVER_URL=https://hope.gate.test`, `HOPE_TRUSTED_PROXIES` set to Caddy's address, `HOPE_AGENT_CA` pointing at the CA file copied to each target, read-only root, `NET_RAW` only |
| Proxy | `caddy:2.10.2-alpine`, `reverse_proxy` to the server with TLS from the throwaway CA, nothing else (no flush or header tuning) |
| Release | `scripts/build-agent-docker.sh` for both Linux musl targets, `scripts/build-agent.sh` for both macOS targets, `scripts/assemble-release.sh` into a temporary releases root mounted read-only into the server. Version 0.1.1 is built from a temporary clone of the tree with only the workspace version bumped, so the repository's version is never changed |
| Proxy gates | TLS only with the CA, Secure/HttpOnly/HSTS/no-store through Caddy, Origin check, SSE latency through Caddy, rate-limit key is the client (two containers, spoofed `X-Forwarded-For` ignored), negative control without `HOPE_TRUSTED_PROXIES` |
| Installer matrix | Per architecture: console command as root with custom CA, reinstall, interrupted upgrade (SIGTERM at `wait-report`), upgrade, downgrade refusal and `HOPE_ALLOW_DOWNGRADE`, uninstall with identity kept, reinstall, purge, non-root with sudo, non-root without sudo (with and without a user manager). Each cell checks the service state, that the running `MainPID` equals the accepted `process_id` of `hope-agent status`, that the binary SHA-256 equals the signed manifest and that the server saw a fresh report |
| SSH jobs | Real sshd, jobs created through the HTTP API: changed host key (sshd saw no authentication attempt), wrong password, password install, key reinstall of the same agent, non-root without sudo (`LINGER_REQUIRED` without lingering; with lingering a user service that still reports 45 s after the session ended), dropped connection (session killed / container killed). Then the database files and server and proxy logs are searched for every password, the private key and every issued enrollment token |

## Limits

Emulated amd64 is Rosetta, not an Intel CPU. Containers share one kernel and one network bridge, so this is not a physical LAN; the lingering and logind behavior is that of Debian 13 only. The CA is trusted by file path, so the default public-CA path is not exercised. Only Linux cells run here.

## Updater gate

`tests/gates/run.sh updater` is a leaner, Linux-only fixture for the agent updater (`docs/operations.md`, "Updating agents from the console"). It needs Docker, OpenSSL, `cargo` and Python 3, and no macOS host: it builds the server and four agents (versions `0.9.1` to `0.9.4`, so none can be mistaken for a real release) for the host's architecture, signs them with throwaway keys into a partial release, serves them from the real server behind `tests/agent/https_proxy.py` with a throwaway CA, and boots a Debian 13 container with systemd as PID 1 (`updater.Dockerfile`, run `--privileged --cgroupns=host --network host`; disposable machines only). Then, with the console's own HTTP API and installation command:

| Cell | Checks |
| --- | --- |
| install | The installer reports `result=installed`; `hope-agent` and `hope-agent-update.path` are active and enabled; the queue directory is `hope-agent:hope-agent 0700`; the updater and pinned installer are root-owned and not writable by the agent account; the server sees the agent and its working updater |
| update all | Publishing a newer signed release and asking for updates replaces the agent through the real path unit and `hope-agent-update.service` (`Result=success`): installed SHA-256 equals the signed manifest, the new `MainPID` runs the installed binary, the request file and transaction are gone, `update-result` says `updated`, the server clears the request and shows "Updated to ..." |
| no-op | A request written by the agent account (`runuser`) for an up-to-date agent is consumed and answered `current`; the service is not restarted |
| wrong key | The release directory is switched to one signed by another key and an update is requested: the pinned installer refuses it (`MANIFEST_INVALID`), the agent and its `MainPID` are untouched, and the failure reaches the console |
| rollback | A correctly signed release is installed, then the server becomes unreachable at `phase=wait-report`: the updater times out (`FIRST_REPORT_TIMEOUT`), rolls back, the previous binary runs again, no transaction is left, and once the server returns the agent reports and the console shows the failure |
| uninstall | The pinned installer's `uninstall` removes the agent, the updater script, the pinned installer and both units |

Variables: `HOPEGATE_UPDATER_IMAGE` (a prebuilt image providing systemd as PID 1, `curl` and `openssl`, instead of building `updater.Dockerfile`), `HOPEGATE_WORK`, `HOPEGATE_KEEP=1`, `HOPEGATE_SKIP_BUILD=1`, `HOPEGATE_TLS_PORT` and `HOPEGATE_BACKEND_PORT` (default 18443 and 18080). CI runs it as the `updater-gate` job. Limits: one distribution (Debian 13, systemd 257), one architecture per run, `--network host`, no launchd and no user-mode (no updater is installed there by design).
