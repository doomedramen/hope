# Agent operations validation — 27 September 2026

All live checks used disposable PostgreSQL 17, a dedicated server on port 8088/18443, temporary release keys, and a native Linux arm64 systemd container. No production agent or database was upgraded. Test-only releases are not published project releases.

## Demonstrated behavior

| Check | Result |
| --- | --- |
| Fresh PostgreSQL workspace suite | 300 tests passed; two existing opt-in checks ignored. Migrations and database-backed tests ran with `DATABASE_URL` set. |
| Fleet search and pagination | A database test found all 101 matching agents across pages, with accurate online/offline totals and no overlapping page IDs. Never-connected agents are not reported online. |
| Frontend suite | 82 tests passed, including paused log stability, literal log rendering, filter propagation, per-agent bulk results, and current versus stale overview states. |
| Native installer | Signed arm64 0.1.1 installed and enrolled. The normal agent service runs as `hope-agent`; the updater is a separate root-owned service and timer. |
| Journal and Docker logs | Both selected sources produced searchable records. Repeated lines stayed distinct. Literal `SECRET-MARKER` text was redacted before delivery. Permission/CLI failures produced agent diagnostics. |
| Managed update | Signed 0.1.1 → 0.1.2 succeeded through the outbound connection. Identity stayed unchanged; metrics and both previously healthy log sources resumed. |
| Failed startup | A signed, valid arm64 ELF test executable that exits immediately failed the 120-second local health deadline. The helper restored 0.1.2, confirmed local collection, and the server recorded `rolled_back`. |
| Tampered staged bundle | Appending bytes after staging caused the root helper to reject the signed-size mismatch. The installed executable's SHA-256 stayed unchanged and the service remained active. |
| Disconnection and restart | A short real Docker network outage included an agent process restart. Captured outboxes contained 7 metric samples and 136 distinct log events. After reconnect, all appeared exactly once in PostgreSQL. |
| Initial mobile viewport | At 375 × 812, health, CPU/memory, freshness, log status, update state, and next action fit without scrolling. No horizontal overflow. Disconnected readings were labeled historical. |
| Build gates | Rust formatting/Clippy, web lint/build, helper tests, Linux cross-compile, Rust tests, and web tests passed through Git hooks. |

## Reproduction

Run the normal automated checks against an empty disposable database:

```sh
DATABASE_URL=postgres://USER:PASSWORD@HOST:PORT/DISPOSABLE_DB \
  cargo test --workspace -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
pnpm -C apps/web test -- --run
pnpm -C apps/web build
```

For live update checks, follow the signing and server setup in `.github/workflows/ci.yml`, using temporary signing keys and a disposable release repository. Build two agents with the same trusted public key and different `HOPE_AGENT_RELEASE_VERSION` values. Enroll the first through the installer, enable the chosen sources, publish the second signed bundle, and request the update from Overview. Verify operation state, agent identity/version, fresh metrics, and successful source polling.

Use `apps/web/e2e/agent-target/Dockerfile` for the Linux systemd target. On the Docker Desktop host used here, native architecture and `--cgroupns=host` were required for journald; the emulated amd64 systemd target failed before agent execution. Install the Docker CLI/engine inside the disposable target for container logs, and explicitly grant its service account journal/Docker access. These are test-environment permissions, not installer defaults.

Exercise rollback only on a disposable target, with a deliberately failing but correctly signed native ELF artifact. For staged tampering, stop the updater timer, stage a valid signed bundle under the fixed update directory, alter the staged executable, start the helper once, and compare the installed binary hash and failure status. Do not perform failure injection on a real monitored host.

For replay checks, disconnect only the disposable target's Docker network, restart its agent, capture the queued metric sample IDs and log event IDs, reconnect, and query for those exact IDs. Require one stored record per retained identity after the backlog drains.

## Remaining operational gates

The live outage was short, not the planned 30 minutes. Production-size mixed telemetry/log load, reconnect storms, storage sizing, restore drills, and five-operator usability sessions remain unverified. The implementation plan lists additional controls and longer history views separately. These checks do not establish a 500-agent production capacity guarantee.
