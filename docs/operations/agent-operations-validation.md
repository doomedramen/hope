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

## Follow-up acceptance

Additional implementation and checks completed in the same disposable environment:

| Check | Result |
| --- | --- |
| Automated suite | 305 Rust tests passed with real PostgreSQL; five opt-in tests are ignored in the normal suite. All three agent capacity tests passed when run explicitly, including the ten-minute TLS workload. 85 UI tests passed; build, formatting and Clippy passed. |
| Enrollment correlation | A native Linux test agent completed authenticated Hello, inventory and metrics milestones in about five seconds. A database test proves a second attempt remains incomplete and milestones survive token cleanup. |
| Thirty-minute outage | Network disconnected for 1,805.22 seconds, with the agent restarted after one minute. All 121 captured metric sample IDs and 2,704 captured log event IDs arrived exactly once. |
| Update during catch-up | Signed test release 0.1.2 → 0.1.5 installed after reconnection, preserving identity and outboxes; server health verification succeeded. Acknowledged backlog now advances every 100 ms while pending acknowledgements retain retry backoff. |
| Long history | Database tests verify 180-day aggregation uses weighted counts, preserves extrema and the latest value. The UI offers 30- and 180-day ranges and labels aggregation resolution. |
| Initial Settings viewport | At 375 × 812, source selection, applied revision and Save are visible; Save spans pixels 312–356. Document width equals viewport width. Secondary tabs prioritize their own task rather than repeating Overview. |
| Overview tab layout | Health and current readings now belong to the Overview tab panel, below the stable tab bar. The former instruction-only panel is removed. A regression assertion verifies the panel contains the summary and follows the tab list; desktop and mobile initial views were checked. |
| Real inventory shape | Structured socket reachability endpoints render correctly; the previous Settings crash is covered by the UI fixture. |
| Capacity and restore | Three local 500-agent workloads and a full-row restore comparison passed. The ten-minute TLS workload preserved 20,000 metrics and 570,000 logs through 1,500 reconnects and ten retention/checkpoint cycles. See the dedicated report for timings, storage allocation and scope limitations. |

Native journal source controls passed: with minimum severity warning and a five-record/minute budget, 20 warning and 20 info messages produced exactly five stored warnings, zero stored info messages, and a durable diagnostic accounting for the other 15 warnings. The agent acknowledged the revision; the original test source selection was restored afterwards. [Source-policy evidence](validation/agent-source-controls-2026-09-27.json).

[Outage evidence](validation/agent-outage-2026-09-27.json), [capacity and restore report](agent-capacity-validation.md), and [resource sample](validation/agent-resource-sample-2026-09-27.json).

During a 60-second disconnected-collection sample, the native agent cgroup used 0.34% of one CPU and peaked at 32,247,808 bytes (30.75 MiB). These systemd counters include supervised child processes and represent one fixture, not a regression budget or universal baseline.

## Remaining operational gates

Initial [physical storage sizing](agent-storage-sizing.md) is complete: the default-rate 500-agent workload needs about 346 GiB of planning capacity after measured table/index costs, fixed rollup windows and explicit maintenance/WAL allowances. That does not fit the current machine's free disk. Full-retention deployment acceptance remains open until storage or data requirements are agreed and validated; no collection or retention settings were changed.

The agreed local 500-agent ten-minute TLS throughput/recovery check passed, including retention/checkpoint pressure. It does not include production network latency, full retained history or a multi-hour soak. Five real operator usability sessions also remain open. The requested [self-guided study pack](agent-study-pack/START-HERE.md) includes participant tasks/forms, isolated-fixture instructions, a results sheet and a report template. No human results have been invented. Per-source and per-connection log bounds are not a fleet-wide quota.
