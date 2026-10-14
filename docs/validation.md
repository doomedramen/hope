# Validation evidence

This records observed results, not promised performance. Reference environment on 2026-10-08: Apple M4 Pro, 24 GiB host memory; OrbStack Linux ARM64 VM, 12 logical CPUs and approximately 12 GiB VM memory; Docker 29.4.0; Rust 1.96.0; Node 26.8.2/npm 11.19.1.

## Build and contract checks

Baseline milestone `9f326de`: native formatting, Clippy, six Rust tests, frontend typecheck/lint/export, three Chromium acceptance tests, and Linux ARM64 release workspace compilation passed. Type-generation failure injection preserved the previous output and removed the temporary file.

Contract milestone `90db4ed`: shared API DTOs, enum compatibility, bounded report validation, legacy host defaults, and transactional database adoption passed native checks. Future database versions are rejected instead of being overwritten.

Discovery changes: native and Linux ARM64 workspace formatting, tests, Clippy, and release image build pass. Tests cover fairness across two silent `/24`s, cancellation, TCP refusal, shared probe concurrency/start limits, 250-host enrichment, bounded DNS deadlines/output, and ARP frame validation. Browser acceptance and installation/resource gates are tracked separately below.

## Discovery fixture

Reproduce with:

```sh
docker build --target checks -t hope:linux-checks .
docker build -t hope:validation .
python3 scripts/benchmark-discovery.py --runs 5 --output target/discovery-routed.json
python3 scripts/benchmark-discovery.py --runs 5 --on-link --output target/discovery-on-link.json
```

The fixture creates disposable internal Docker bridges for `198.18.1.0/24` and `198.18.30.0/24`. Each has one HTTP responder whose ICMP input is blocked. Other addresses are silent except fixture infrastructure. In routed mode, the server attaches to the first bridge and reaches the second through a forwarding container at `.252`. An unrelated management interface exposes the test API to localhost; no host LAN is scanned. Cleanup removes fixture containers and networks even after failure.

Settings: 64 concurrent host jobs, 64 concurrent probe operations, shared ceiling of 200 probe starts/second; ICMP/TCP deadlines 750 ms, on-link ARP deadline 250 ms, host budget six seconds. Enrichment has eight workers and a bounded deduplicated queue. The start-rate ceiling controls scheduled probe operations; TCP handshakes/teardown and kernel neighbor resolution can generate additional frames. It is not an absolute interface packet policer.

Before tuning, all five directly attached runs published responders in 0.77–0.83 seconds but completed in 30.45–30.50 seconds, failing the 30-second target. Reducing only the local ARP deadline released slots earlier without increasing concurrency or the probe-start ceiling. Both final five-run fixtures passed. Results below are wall-clock API observations; raw files include the image ID and per-range persisted timings.

| Fixture | Runs | First responder (seconds) | Both passes complete (seconds) | Targets |
| --- | --- | --- | --- | --- |
| [Second range routed](benchmarks/discovery-routed.json) | 5 | 0.787–0.817 | 25.397–25.437 | Pass |
| [Both ranges on-link](benchmarks/discovery-on-link.json) | 5 | 0.772–0.806 | 26.392–26.464 | Pass |

## Gates still open

This list is historical. The current consolidated state is the [Release gate status](#release-gate-status) table at the end of this file.

- Discovery frame/cost measurements; completion targets passed in both controlled fixtures.
- Remaining report/retention boundary, restart, association, and 250-host retention load checks.
- Agent delivery fault tests and 30-minute RSS/CPU measurements.
- Expanded browser behavior and visible interactive review.
- Transactional installer harness, actual systemd/launchd service matrix.
- Isolated SSH provisioning integration tests.
- Ticket 09 gates are recorded below; open there: real-host HTTPS reverse proxy (Caddy/nginx) behavior, Docker Desktop on Windows, and the full-LAN Linux host-network run.
- Signed artifacts for all four target platforms and protected release CI.

Never treat synthetic browser inventory as discovery evidence, command mocks as real service evidence, or cross-compilation as installation evidence.

## Paused data-integrity checkpoint

Ticket 04 is partially complete. Native checks pass: 24 Rust tests, Clippy, formatting; shell installer syntax; frontend typecheck/lint/static export. Regression tests cover lost enrollment responses, concurrent token reuse, revocation/report ordering, identity-bound credential replacement, weighted mixed-resolution history, and late-sample retention idempotence. The Linux image, browser acceptance, 250-host load, and remaining boundary/association tests have not been rerun for this checkpoint. See the handoff's pause checkpoint before resuming.

## Retention and association — 2026-10-09

Native and Linux ARM64 checks pass: 28 Rust tests, formatting, and Clippy. The explicit release load test also passes on both platforms. It seeds 250 identities with 24 hours of raw samples, six days of five-minute aggregates, and 2,500 late samples: 1,154,500 stored samples, approximately 538 MiB. It measures the report handler and simulated discovery persistence while retention runs; HTTP transport, collectors, and actual network probing are outside this load fixture.

| Platform | Cleanup calls | Cleanup seconds | Concurrent reports | Report median ms | Report maximum ms |
| --- | --- | --- | --- | --- | --- |
| [macOS ARM64](benchmarks/retention-macos-arm64.json) | 2 | 0.226 | 13 | 0.868 | 2.140 |
| [Linux ARM64 VM](benchmarks/retention-linux-arm64.json) | 2 | 0.552 | 25 | 3.064 | 14.036 |

Reproduce with `cargo test --release --locked -p hope-server --test retention_load -- --ignored --nocapture`, or build Docker target `load-checks`. The native compiler is Rust 1.96.0; the Linux image uses 1.96.1. OrbStack's default BuildKit DNS failed to resolve the Rust component host during this run; `docker build --network=host --target load-checks -t hope:load-checks .` completed. This changes only the validation build's network mode.

Boundary/restart tests exposed and fixed deletion of a partially retained five-minute bucket. Rollups now expire only when their full interval is older than seven days; history queries include overlapping retained buckets. Tests also verify unique-MAC enrollment, refusal to associate by IP alone, ambiguous-MAC manual reconciliation, preserved history, qualified map-position cleanup, and repeated ambiguous discovery without duplicate growth.

Release builds on macOS exposed a malformed stripped build-time proc-macro library. Build dependencies now retain symbols while shipped release binaries retain stripping, using Cargo's [build-profile overrides](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides). Unused SQLx default features were removed. The native release load test then compiled and passed.

## Agent delivery milestone — 2026-10-09

macOS ARM64 (Rust 1.96.0) and Linux ARM64 in OrbStack (Rust 1.96.1): `cargo fmt --all -- --check`, `cargo test --workspace --locked`, and workspace/all-targets/all-features Clippy with warnings denied pass. Linux command: `docker build --network=host --target checks -t hope:checks .`. There are 42 ordinary tests; the million-sample retention benchmark remains explicitly ignored in ordinary runs. `make types`, frontend typecheck, and static production export pass.

Agent tests demonstrate continued sampling during a pending ten-second response using accelerated test cadence; bounded count/bytes/age/future skew; corrupt/oversized startup queues; duplicate state ownership/private atomic replacement; command deadline/output caps; one hanging filesystem worker; counter reset; distinct GPU identifiers; inventory suppression after acceptance; offline startup; TLS handshake failure against a plaintext fixture; rejected, throttled, oversized, or false-success responses; and accepted-only markers. The TLS fixture is not a real custom-CA or invalid-certificate matrix. Accelerated tests are not a measured normal-cadence or CPU/RSS benchmark. Those remain unverified.

Network totals select the default-route interface when known; otherwise they exclude common virtual interface prefixes. All exposed interface counters retain a flag identifying total membership. This fallback is a documented heuristic, not complete topology inference. GPU adapters without a native stable ID remain snapshots when aggregating; unavailable utilization stays null.

## Console/map/history milestone — 2026-10-09

43 ordinary Rust tests pass on macOS ARM64 and Linux ARM64, with formatting and all-targets/all-features Clippy (warnings denied). Linux validation: `docker build --network=host --target checks -t hope:checks .`. Frontend formatting/typecheck/lint/static export pass. Nine Chromium cases pass via `npm --prefix web run test:e2e`; fixtures are separate from the interactive preview.

The added server regression runs 30 concurrent atomic map position edits, retries a manual link without duplication, rejects conflicting link identifiers and stale whole-map replacements, and retains positions through removal. Browser checks exercise actual dragging, simulated failed saves and retry, per-mode persistence/reload, viewport fitting after measured dimensions, manual-link double-click removal, back navigation, overlapping subnet filters, null memory, full-window chart gaps, session expiry, mobile logout, missing packages, and saved-network scan retry. Login sessions are reused between serial cases; the suite does not disable login throttling. Synthetic scan-retry transport failures do not launch real probes.

Desktop and mobile screenshots (`web/test-results/hope-{subnets,health,mobile}.png`) were reviewed after awaiting settled node bounds. The visible Codex preview is at localhost:8088; it retains separately seeded demo records and subsequently added interactive records. Do not delete that database as test cleanup. Availability currently checks packaged files; release manifests, signature metadata, custom-CA/service installation, and native GPU/performance measurements remain open gates.

## Service confirmation foundation / authorized LXC — 2026-10-09

Native macOS ARM64 and Linux x86_64 (Docker Rust 1.96 bookworm build) agent formatting/tests/Clippy pass: 15 unit tests plus one real CLI enrollment-retry integration test. Linux command: `docker build --network=host --platform linux/amd64 --target checks -f tests/agent/Dockerfile -t hope:agent-amd64-checks .`.

The user supplied a throwaway Debian 13.7 x86_64 LXC with one available CPU and 512 MiB RAM. Its host fingerprint was verified before credentials were sent. The authorized web terminal restored a failed SSH service; pinned-key password SSH then succeeded. A dedicated minimum-privilege reporting fixture received an accepted HTTPS report with matching service PID and activation UUID. Its signed binary SHA256 is `3c287663f5de2d8c09b4102135aa5740f2b227eb5566da37204106b346b94fea`.

The fixture uses `tests/agent/{Dockerfile,https_proxy.py,lxc_probe.py,resource_sample.py}` and a separate test CA, signing key, database, and loopback-only reverse tunnel. Private keys and credentials are excluded from Git. Python's strict certificate verification initially rejected a leaf missing Authority Key Identifier; the certificate was corrected and strict verification retained. No insecure TLS bypass is used. The Python proxy is a bounded test transport, not the production reverse-proxy/SSE validation gate. The service was manually provisioned by the fixture; production installer correctness remains unverified. Resource measurement subsequently completed; results follow.


## CPU/memory deployment milestone — 2026-10-09

The authorized Debian LXC now runs a persistent, enabled `hope-agent.service` as the dedicated unprivileged `hope-agent` user. The signed x86_64 binary above reports into the interactive preview database. The accepted service report matched PID 6918 and activation UUID; successive reports contained CPU utilization and memory used/total. The browser showed `debian` at the supplied container address, reporting status, CPU percentages, 512 MiB inventory RAM, and live memory percentage. The separate benchmark service was stopped after measurement, leaving one reporting daemon. Discovery lacks a MAC on this development server, so the discovered IP entry remains separate pending explicit reconciliation.

`lxc_probe.py --visible-site --server URL --enrollment-token-env NAME` is an initial-install validation helper for the explicitly authorized disposable LXC. It refuses existing installations, verifies the downloaded signature over trusted HTTPS, configures enrollment separately, installs the systemd unit, and waits for an accepted report from that service PID and activation. Tokens and SSH passwords arrive through environment/stdin and are not persisted in job output. It is not the production installer, rollback, or upgrade acceptance gate.

The visible deployment currently uses an explicit test CA and a loopback SSH reverse tunnel to the local HTTPS proxy on port 9444. The service survives container restart; the reporting transport still depends on the development workstation and tunnel. A permanent reachable HTTPS endpoint and canonical URL remain deployment work. The generated website installation command currently contains the development HTTP canonical URL and therefore cannot be used unchanged with the HTTPS installer.

[Measured Linux x86_64 agent resources](benchmarks/agent-linux-x86_64-lxc.json): 60-second warmup followed by 1,800 seconds, one-second polling, Intel Core i5-8500T and one available CPU, Debian 13.7 LXC with 512 MiB RAM. Median parent RSS 4,825,088 bytes (4.60 MiB), sampled peak 4,923,392 bytes (4.70 MiB); CPU including waited collector children 0.015% of one core. The result is for the exact recorded binary, normal reporting and this container only. Parent RSS excludes child memory and may miss sub-second peaks. GPU inventory is exposed but driver utilization is unavailable. It does not establish macOS, other architectures, outage behavior, collector costs, or all GPU backends.

Native agent checks after the configuration validation/rollback helpers: 16 unit tests and one CLI integration test pass, with workspace formatting and all-targets/all-features Clippy (warnings denied). Settings rollback retains the server-issued credential and pending retry identity, and rejects a snapshot from another identity/server. Production installer integration of these helpers remains open.

## Operational hardening (Ticket 09) — 2026-10-14

Base commit `acdf128` plus the uncommitted Ticket 09 working tree. Hosts: macOS ARM64 (Rust 1.96.0, Node 26.8.2) for native checks; OrbStack Linux ARM64 VM (Docker, image `hope:t09`, id `sha256:bfd8c2a3…`, built with `docker build --network=host -t hope:t09 .`) for container checks. Browser: Playwright Chromium.

| Command / procedure | Environment | Date | Base | Result |
| --- | --- | --- | --- | --- |
| `make types` | macOS | 2026-10-14 | acdf128 | Pass; generated files unchanged |
| `cargo fmt --all -- --check` | macOS | 2026-10-14 | acdf128 | Pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | macOS | 2026-10-14 | acdf128 | Pass |
| `cargo test --workspace --locked` | macOS | 2026-10-14 | acdf128 | Pass: includes 13 `hardening` tests, SIGTERM binary test, SSH shutdown test, limiter unit tests (`ssh_install` 20) |
| `python3 -I -m unittest discover -s tests/installer` | macOS | 2026-10-14 | acdf128 | Pass, 56 tests |
| `npm --prefix web run typecheck`, `lint`, `format:check`, `build` | macOS | 2026-10-14 | acdf128 | Pass |
| `npm --prefix web run test:e2e` | macOS, Chromium | 2026-10-14 | acdf128 | Pass, 10 tests (new: hardening headers, no CSP violation, hydration under the policy) |
| `docker compose config -q` for `compose.yaml` and `compose.desktop.yaml` | macOS Docker | 2026-10-14 | acdf128 | Pass |
| Image build `docker build --network=host -t hope:t09 .` | OrbStack Linux ARM64 | 2026-10-14 | acdf128 | Pass |
| Desktop compose (`compose.desktop.yaml`, image override) | OrbStack | 2026-10-14 | acdf128 | Healthy; uid 10001; `CapEff` = `NET_RAW` only; capabilities `arp:false, icmp:true`, limitation text names Docker Desktop |
| Host-network compose (`compose.yaml`, `HOPE_BIND=127.0.0.1:18081`) | OrbStack | 2026-10-14 | acdf128 | Healthy; capabilities `arp:true, icmp:true`; health check followed the non-default port |
| Persistent restart (`compose restart`, then stop/start) | OrbStack | 2026-10-14 | acdf128 | Network, password and sessions-by-login persisted in the named volume |
| `docker stop` with a running 4,094-address scan and open SSE stream | OrbStack | 2026-10-14 | acdf128 | Exit 0 in 0.12 s; stream closed by server; scan `interrupted` after restart |
| `hope-server backup` via `docker compose exec` (read-only rootfs) | OrbStack | 2026-10-14 | acdf128 | Wrote 0600 file; live `hope.db` was 4 KiB with 170 KiB of data in `-wal`, confirming why a main-file copy is unsafe |
| Discovery fixture on-link, 1 run: `scripts/benchmark-discovery.py --image hope:t09 --runs 1 --on-link` | OrbStack, disposable bridges `198.18.1.0/24`, `198.18.30.0/24` | 2026-10-14 | acdf128 | First responder 0.78 s, both ranges complete 26.39 s (target 30 s) |
| Discovery fixture routed, 1 run | OrbStack | 2026-10-14 | acdf128 | First responder 0.76 s, complete 25.44 s |
| HTTPS via `tests/agent/https_proxy.py` with a private test CA, container behind it | macOS proxy, OrbStack container | 2026-10-14 | acdf128 | Secure cookie + HSTS observed; `curl` and `hope-agent enroll` without the CA fail (`UnknownIssuer`); with `HOPE_CA_CERT` enroll and first report accepted; cross-origin POST 403; SSE event relayed and closed on `docker stop` |
| `npm --prefix web audit` | macOS | 2026-10-14 | acdf128 | 9 high, all `braces`<=3.0.3 via `fast-glob`/`micromatch` in dev tooling (`eslint-config-next`, `shadcn` CLI). No patched `braces` exists; the only offered fix is a major downgrade (`shadcn@1.0.0`, `eslint-config-next@14`), not applied. Not part of the static export or the server runtime |
| `cargo audit` before / after | macOS | 2026-10-14 | acdf128 | Before: 7 (russh and russh-cryptovec RUSTSEC-2026-0153/0154 high; rustls RUSTSEC-2026-0346..0349; rsa RUSTSEC-2023-0071). After `cargo update -p rustls` (0.23.46) and `russh` 0.54.5 -> 0.60.3: 1 remaining, `rsa` Marvin timing attack, no upstream fix, reached only through russh's RSA key support |
| `cargo report future-incompatibilities` | macOS | 2026-10-14 | acdf128 | russh 0.54.5 warned (`macro_export` absolute path lint). Gone in 0.60.3; no warnings remain |

Unit coverage added: login/enrollment/report limiters (per-client isolation from the global ceiling, bounded key table), trusted-proxy `X-Forwarded-For` handling, same-origin checks for POST/PUT/PATCH/DELETE (`Origin`, `Sec-Fetch-Site` cross-site and same-site), cookie flags with http and https canonical origins, session expiry and logout invalidation, `no-store`/`Pragma` on API responses including errors, fixed error text that never quotes request bodies, SSE `resync` after broadcast overrun, subscriber release on client drop, stream closure on shutdown, consistent backup during concurrent paired writes with restore through `State::open`, and SIGTERM of the real binary.

Unverified or limited:

- The Linux discovery fixture ran with one run per mode on an OrbStack VM, not the five-run protocol, and not on a real LAN with `network_mode: host`. Host networking on Docker Desktop for Mac is a VM network; it says nothing about physical-LAN ARP. The `arp:true` report above is the VM's virtual interface only.
- Docker Desktop for Windows and Docker Engine on a physical Linux host were not tested.
- TLS was tested with a Python test proxy on macOS, not Caddy/nginx/Traefik. Proxy buffering, HTTP/2, idle timeouts and `X-Forwarded-For` handling in real proxies are unvalidated; `HOPE_TRUSTED_PROXIES` was exercised only in-process.
- The `rsa` advisory and the `braces` advisory remain open (see above).
- Backup was exercised in-process and with the CLI in a container; restore procedure was exercised through `State::open` on the backup, not by replacing the file inside a stopped container volume.
- Browser SSE reconnect timing after a server restart was not measured in Chromium.

### Ticket 09 review follow-up — 2026-10-14

Rate limits redesigned so attackers cannot lock out the administrator or agents: no global ceilings at all; failures-only per-client budgets (success consumes nothing, a successful login clears the client); password-hash concurrency semaphore (2 permits, 2 s wait, 503 + `Retry-After`); eviction (expired, then least recently hit) instead of an overflow bucket; IPv6 /64 keys and IPv4-mapped unwrapping before key and trusted-proxy matching; loopback trusted by default when `HOPE_BIND` is loopback-only; one-time warnings for ignored/unparseable `X-Forwarded-For`; normalized Origin comparison; shutdown waits for probe/enrichment tasks before closing the pool; shutdown races closed for scan and SSH job creation; backup fsyncs file and directory.

| Command | Environment | Date | Base | Result |
| --- | --- | --- | --- | --- |
| `cargo fmt --all -- --check`, clippy `-D warnings`, `cargo test --workspace --locked`, `make types` | macOS | 2026-10-14 | acdf128 | Pass (hope-server: 22 lib, 19 hardening, 20 ssh_install, 1 shutdown; types unchanged) |
| `python3 -I -m unittest discover -s tests/installer`, web typecheck/lint/build, `test:e2e` | macOS | 2026-10-14 | acdf128 | Pass (installer 56; web typecheck/lint/build; e2e 10) |
| Container smoke on rebuilt `hope:t09` (desktop compose, project `hopet09r`) | OrbStack | 2026-10-14 | acdf128 | Healthy, 12 consecutive successful logins from one client, `docker stop` exit 0 in 0.15 s; project removed |

New tests: 4 limiter unit tests (failure-only budgets, eviction with legitimate keys surviving, IPv6 /64 + v4-mapped, loopback default), and in `hardening.rs` distributed failures vs. administrator and free successes, hashing saturation 503, valid agents under distributed bad credentials, normalized Origin, startup reconciliation of running scans, and refusal after shutdown begins. Not tested: the exact interleaving of a scan/SSH-job insert racing `settle` (a deterministic test needs injected delays); the post-insert recheck is simple and covered only by the refusal path.

## Signed release matrix and CI (Ticket 10) — 2026-10-14

Base commit `f29f38e` plus the uncommitted Ticket 10 working tree. Host: macOS ARM64 (Rust 1.96.0); Linux targets built in OrbStack Docker (`rust:1.96-bookworm`, `musl-tools`) through `scripts/build-agent-docker.sh`. The signing key was a throwaway 3072-bit RSA key generated in a scratch directory outside the repository; no production key exists or was used. The repository `releases/` directory was not touched.

### Artifacts built (all `0.1.0`, `--locked`, release profile)

| Platform | Target | Size (bytes) | SHA-256 | Minimum OS / libc (measured unless noted) | Run here |
| --- | --- | --- | --- | --- | --- |
| linux-x86_64 | `x86_64-unknown-linux-musl` | 4,339,720 | `7c74763aaafa69ff4b78b41a168d7473cf39a94ff81776c1651763a36620f067` | static-pie, no `NEEDED`, no `INTERP`: no glibc or loader required. Kernel floor 3.2 is Rust's documented minimum for the target, not measured | `--version` under `linux/amd64` emulation: `hope-agent 0.1.0` |
| linux-aarch64 | `aarch64-unknown-linux-musl` | 3,741,168 | `5ef11c0284c3c879d18f3492d05863749eae77017ed6dc2deb05d2537334ec99` | statically linked, no `NEEDED`. Kernel floor 4.1 is Rust's documented minimum, not measured | `--version` in a native `linux/arm64` container: `hope-agent 0.1.0` |
| macos-x86_64 | `x86_64-apple-darwin` | 3,803,616 | `52cd2c18b2ca06efd427f524fd5141674267227b3d8c77db819eb2fd59ef1fc0` | `LC_BUILD_VERSION minos 11.0` (`MACOSX_DEPLOYMENT_TARGET=11.0`) | Not run: no Rosetta on this Mac (`arch -x86_64` fails) |
| macos-aarch64 | `aarch64-apple-darwin` | 3,545,360 | `57d54a62115c5415dd4ab97c7e57a72f186b20eb940ba0a6c143f26d59630ccd` | `LC_BUILD_VERSION minos 11.0`; links only `libSystem`, `libobjc`, `libiconv`, CoreFoundation, IOKit | `--version`: `hope-agent 0.1.0` |

Linux libc decision: static musl. The agent uses `reqwest` with rustls (ring); `cargo tree` shows no `openssl-sys`/`native-tls`, so musl links cleanly, the binary has no glibc floor, and one artifact runs on any distribution including Alpine. The `-gnu` triples still build (`build-agent.sh` reads the required glibc from `objdump -T`) but are not in the matrix. Not measured: behaviour of musl's resolver on hosts with unusual `nsswitch`/`resolv.conf` setups (musl does not use NSS) and the earlier agent resource measurements predate the musl artifact and have not been repeated with it.

### Commands

| Command / procedure | Environment | Date | Base | Result |
| --- | --- | --- | --- | --- |
| `scripts/build-agent.sh aarch64-apple-darwin` / `x86_64-apple-darwin` | macOS ARM64 | 2026-10-14 | f29f38e | Pass; minos 11.0 verified against the pinned target; arm64 smoke test passes, x86_64 not executable here |
| `scripts/build-agent-docker.sh aarch64-unknown-linux-musl` / `x86_64-unknown-linux-musl` | OrbStack (native arm64; amd64 emulated) | 2026-10-14 | f29f38e | Pass; static check and `--version` smoke test pass |
| `scripts/assemble-release.sh` with the four artifacts and a throwaway key, then `openssl dgst -sha256 -verify` of the manifest and every artifact, `shasum -a 256` against the manifest | macOS | 2026-10-14 | f29f38e | Pass; `manifest.json` lists 4 artifacts, `partial:false`; all signatures verify |
| Refusals observed: key inside the repo, missing platforms, mixed versions, different key without `--rotate-key`, `--rotate-key --allow-partial`, older version; rotation re-signed all four artifacts and kept the old release as `releases.prev` | macOS | 2026-10-14 | f29f38e | Pass (also covered by `tests/release`, 6 tests) |
| `python3 -I -m unittest discover -s tests/installer` | macOS | 2026-10-14 | f29f38e | Pass, 71 tests (56 existing plus 15 manifest/checksum/downgrade tests) |
| `python3 -I -m unittest discover -s tests/release` | macOS | 2026-10-14 | f29f38e | Pass, 6 tests |
| `cargo fmt --all -- --check`; clippy `--workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --locked` | macOS | 2026-10-14 | f29f38e | Pass; 104 tests (6 new `releases` unit tests, 2 new `tests/releases.rs`) |
| `make types` | macOS | 2026-10-14 | f29f38e | Pass; `web/lib/protocol.ts` unchanged |
| `npm --prefix web run typecheck`, `lint`, `build` | macOS | 2026-10-14 | f29f38e | Pass (no web source changed; e2e not rerun) |
| `sh -n scripts/*.sh`; `shellcheck -s sh scripts/*.sh` (0.11.0, brew) | macOS | 2026-10-14 | f29f38e | Pass after fixing 4 findings in `install-agent.sh` and 1 in the new scripts |
| `actionlint` 1.7.12 on `.github/workflows/*.yml` | macOS | 2026-10-14 | f29f38e | Pass |

### Unverified or limited

- **Real installation matrix is UNVERIFIED for every cell.** No systemd or launchd service was installed, upgraded, interrupted or uninstalled with a real artifact on any OS/architecture, in system or user mode, with default or custom CA. The installer harness uses fake service managers; cross-compilation and `--version` do not count. The `install-matrix-privileged` job in `ci.yml` is dispatch-only and intentionally fails until those tests exist. Do not claim platform support from this document. This step needs the user's authorization and disposable hosts.
- `x86_64-apple-darwin` was never executed (no Rosetta here). Its minos and size were inspected only.
- The GitHub workflows were linted with `actionlint` but never run. Runner labels (`ubuntu-24.04-arm`, `macos-14`, `macos-15-intel`; `macos-13` named in the ticket has been retired by GitHub), Node 24 and action versions are unverified. Actions are pinned to major version tags, not commit SHAs, because SHAs could not be verified offline; pin them before first use.
- The `release` environment, required reviewers, the tag restriction and the `HOPE_SIGNING_KEY` secret are repository settings that do not exist yet.
- The Linux discovery fixture and Playwright jobs in CI are copies of the local procedures and have not run on GitHub runners.
- Server serving of the real manifest was tested with generated fixtures in temp directories, not by pointing a live server at the assembled directory. The installer's manifest parser was tested against harness-generated manifests in the same layout as `assemble-release.sh` (and its `sed` expressions were run by hand against the real `manifest.json`), not by running the installer end to end against the real artifacts.
- Publishing swaps a `current` symlink (see review follow-up below). The GNU `mv -T` branch was only checked by hand in a Debian container; the macOS `mv -h` branch is covered by `tests/release`.

### Ticket 10 review follow-up — 2026-10-14

- `assemble-release.sh`: the validated absolute key path is now what signs (it was a caller-relative path after `cd`); `--staging`/`--out` resolve against the caller's directory; the in-repo check is case-insensitive and also catches a hard link via `find -samefile` (the build `target`/`node_modules` trees are skipped). Versions must be plain semver without `v`. Artifacts must share one git commit (`unknown` only with `--allow-partial`), the manifest records `commit` and per-artifact `target`, and ELF `e_machine` / Mach-O `cputype` must match the platform label.
- Publishing is now `releases/<version>-<random>/` plus an atomically renamed `current` symlink (and `previous`); retention is current plus two older. The server resolves `current` per request; no `current` means the directory is the release. INT/TERM/HUP clean up the temporary directory and exit 1.
- Server: no manifest means no platforms (legacy fallback removed); `key_id` must equal the SHA-256 of the PEM's SPKI DER.
- Installer: `curl --max-filesize` on manifest (64 KiB), signatures (16 KiB) and artifact (manifest size). `release.yml`: tag pushes only, a `guard` job (tag on a commit on main, tag equals crate version), build provenance attestation. `build-agent-docker.sh` passes the host commit and chowns outputs to the invoking user (the container still runs as root to `apt-get install musl-tools`). `package-agent.sh` restricts its staging directory and no longer implies `--allow-downgrade`. `build-agent.sh` tries Rosetta only for x86_64.
- Results (macOS): `tests/release` 13 tests, installer harness 71, `cargo test --workspace --locked` pass (releases unit tests 8, `tests/releases.rs` 3; 107 workspace tests), fmt/clippy/shellcheck/actionlint/`sh -n`/`git diff --check` clean.

## Agent resource benchmarks (Ticket 11)

Date 2026-10-14, base commit `d0af28e` (Rust sources clean; the new scripts and documents are uncommitted). Procedure: `scripts/bench-agent.sh` (builds `--release --locked`, then `scripts/bench-agent.py`). It starts a throwaway `hope-server` (temporary data directory, free loopback ports, empty release and web directories) behind a TLS proxy that counts bytes (private test CA, strict verification), enrolls a foreground `hope-agent run` with a temporary `HOPE_AGENT_DIR`, warms up 60 s, samples the agent once per second for 1800 s, then SIGKILLs the server for 300 s and restarts it, sampling until the spool is empty. Nothing in `./releases`, `./data` or any running server is touched.

Method and its limits (also recorded in the JSON):

- CPU is the agent's user plus system time from the OS (`proc_pid_rusage`), divided by wall time; waited collector children are reported separately and included in the headline figure. The timebase conversion was checked against a 2 s busy loop (2.02 s read).
- Memory is the agent's resident set (RSS) once per second. On macOS RSS includes shared library pages; the physical footprint (private memory) is reported next to it. Child RSS is sampled only once per second, so short-lived collector children are mostly missed (the peak sampled child RSS was 0 during measurement).
- Sampler overhead is this script's own CPU, reported separately (0.025% of one core). The proxy and server are separate processes and are charged to neither.
- Bytes are plaintext HTTP bytes counted after TLS termination: TLS handshake and record overhead are **not** included. The agent itself performs the TLS work, so its CPU/RSS include it. Everything is on loopback; network latency effects are absent.
- Per-report byte figures are approximate: the proxy counts `POST /api/v1/agent/report` occurrences in read chunks (a request line split across two reads would be missed) and counts retried attempts as requests, and the byte totals include HTTP headers and keep-alive reuse. Totals per window are exact for the bytes seen; only the per-request division is approximate.
- `--recovery-max` (default 300 s) bounds how long the script waits for the spool to empty. The agent's backoff can reach 900 s after repeated failures, so a longer outage may exceed the wait: the JSON then records `spool_drained: false` rather than a recovery time. The 300 s outage here recovered in 148 s.
- Collector duration distribution comes from `hope-agent benchmark 120` in a separate process before the window.
- The binary measured is the plain `cargo build --release` output of this tree, not the signed `aarch64-apple-darwin` artifact built with `MACOSX_DEPLOYMENT_TARGET=11.0`.

### macOS arm64 result

Machine: Apple M4 Pro (12 logical CPUs, 24 GiB), macOS 27.0.1 (26A434), Rust 1.96.0. Agent SHA-256 `2d0f2e98b98ee8c268f3ed2662d50f6cc012d167f565b90e80cbbfe2041c0d04`, `hope-agent 0.1.0`. Raw data, per-second samples and the exact method: [agent-macos-arm64-apple-m4-pro.json](benchmarks/agent-macos-arm64-apple-m4-pro.json).

| Metric (1800 s steady window after 60 s warmup) | Result | Target | Verdict |
| --- | --- | --- | --- |
| RSS median / p95 / peak | 14.20 MB / 14.34 MB / 14.39 MB (13.5 / 13.7 / 13.7 MiB) | below 30 MB | Pass |
| Physical footprint median / peak | 5.28 MB / 5.46 MB | informational | |
| CPU, agent only | 0.140% of one core (2.52 CPU-s in 1800 s) | below 0.5% | Pass |
| CPU, including waited collector children | 0.146% of one core | below 0.5% | Pass |
| Sampler overhead (separate) | 0.025% of one core | n/a | |
| Application bytes per report (client to server / server to client) | 5,230 / 644 | informational | |
| Application bytes per hour (both directions, extrapolated from 60 reports) | 705 kB | informational | |

Warmup window (includes startup, enrollment follow-up, the one-off GPU inventory probe): 0.63% of one core including children, RSS 13.5 MB. This window is not part of the headline figure.

Collector costs (120 samples, one process, `hope-agent benchmark`): whole collection median 29.5 ms, p95 63.3 ms, max 385 ms. The dominant recurring cost is `filesystems` (median 25.8 ms, p95 54.3 ms, max 89.8 ms), about 88% of a sample; `gateway` median 2.0 ms and `network_refresh` 1.4 ms follow; CPU, memory, network counters and `inventory` (the per-sample inventory rebuild, which includes the change check input) are each below 0.6 ms at p95. `gpu_inventory` runs once per hour through a child process (`system_profiler`): 289 ms in the one sample where it ran, and it is the cause of the 385 ms maximum and of the extra warmup CPU. If the CPU target were ever missed, `filesystems` is the first place to look; it is not close to being missed here.

GPU: macOS exposes inventory only (name `Apple M4 Pro`); `gpu_metrics` is reported unavailable (`GPU driver metrics are unavailable; inventory remains visible`) and `disk_io` is reported unavailable on this platform. No supported GPU collector backend was available on this machine, so "with and without GPU collectors" could not be compared; the figures above are for a machine where GPU metrics are unavailable.

Disconnected-server phase (server SIGKILLed for 300 s, then restarted):

| Observation | Result |
| --- | --- |
| Agent kept sampling and stayed up | Yes (same process, no restart) |
| RSS median during outage vs steady median | 13.06 MB vs 14.20 MB (no growth); physical footprint 5.75 MB vs 5.28 MB (+0.47 MB) |
| CPU during outage | 0.126% of one core including children |
| Spool at peak | 15 reports, 77,524 bytes (limits are 120 reports, 4 MiB, 1 h; the limits are exercised by unit tests, not by this outage length) |
| Connection attempts during the 300 s outage | 3 (exponential backoff of 60 s, 120 s, ... plus jitter) |
| Recovery after restart | First accepted report and empty spool 148 s after the server was back, all 15 reports delivered; 0.19% of one core during recovery |

Recovery time is therefore dominated by the agent's backoff timer (it grows to 900 s after repeated failures), not by the server's startup. This is by design; it is noted here because an operator restarting a server after a long outage should expect agents to reconnect over the following minutes.

Not measured: Linux ARM64, macOS x86_64, GPU-collector comparison, the musl Linux artifact, and the server's own resource use. The Linux x86_64 LXC result earlier in this file remains the only Linux figure and predates the transactional installer.

## Documentation checks (Ticket 11)

| Command | Environment | Date | Base | Result |
| --- | --- | --- | --- | --- |
| `python3 -I -m unittest discover -s tests/docs` | macOS | 2026-10-14 | d0af28e | Pass, 6 tests: every `fail`/`err` code in `scripts/install-agent.sh` and every code in the `ssh.rs` catalog has a row in `docs/operations.md` (checked to fail when a row is removed), no stale rows, README/docs relative links and anchors resolve |
| `python3 -I -m unittest discover -s tests/installer` | macOS | 2026-10-14 | d0af28e | Pass, 71 tests (unchanged) |
| `python3 -I -m unittest discover -s tests/release` | macOS | 2026-10-14 | d0af28e | Pass, 13 tests |
| `sh -n scripts/*.sh`; `shellcheck -s sh scripts/*.sh` (includes new `bench-agent.sh`) | macOS | 2026-10-14 | d0af28e | Pass |
| `scripts/bench-agent.sh` smoke run (5 s warmup, 70 s measure, 40 s outage) before the full run | macOS | 2026-10-14 | d0af28e | Pass; full run above |
| `git diff --check` | macOS | 2026-10-14 | d0af28e | Pass |
| `cargo fmt`, Clippy, `cargo test` | n/a | 2026-10-14 | d0af28e | Not rerun: no Rust or web source changed in this ticket |

## Release gate status

A release is complete only when all five product stages are evidenced. **Status today: not complete.** Verified means evidenced by a command and result recorded in this file; unverified means no real-environment evidence exists, whatever local tests say.

| # | Stage | Verified (evidence) | Unverified |
| --- | --- | --- | --- |
| 1 | Discovery and hosts | Fair concurrent scanning, cancellation, enrichment and ARP frame validation tests; Linux ARM64 fixtures, five runs per mode in Ticket 03 ([routed](benchmarks/discovery-routed.json), [on-link](benchmarks/discovery-on-link.json)) and one run each in Ticket 09 (see [Operational hardening](#operational-hardening-ticket-09--2026-10-14)); capabilities banner on Docker Desktop mode | Physical LAN with `network_mode: host` and ARP; Docker Engine on a physical Linux host; Docker Desktop on Windows; absolute frame counts; Ticket 09's five-run protocol |
| 2 | Map modes | 10 Chromium tests: modes, drag persistence per mode, manual links, filters, selection across map/list, session expiry, history gaps; screenshots inspected ([Console/map/history](#consolemaphistory-milestone--2026-10-09), [Ticket 09](#operational-hardening-ticket-09--2026-10-14)) | Real discovery data on the map (tests use synthetic inventory) |
| 3 | Agent and history | 250-host retention load ([Retention](#retention-and-association--2026-10-09)); delivery fault tests ([Agent delivery](#agent-delivery-milestone--2026-10-09)); [Linux x86_64 LXC measurement](benchmarks/agent-linux-x86_64-lxc.json); [macOS arm64 measurement and outage test](#agent-resource-benchmarks-ticket-11) (targets met) | Installed service reports on any OS through the production installer; Linux ARM64 and macOS x86_64 measurements; GPU-collector comparison; musl artifact measurement |
| 4 | Manual installation | Transactional installer with rollback: 71 harness tests with stubbed service managers and real signatures ([Ticket 07 notes](IMPLEMENTATION_HANDOFF.md#ticket-07-installer-rewrite-and-harness)); signed manifest verification ([Ticket 10](#signed-release-matrix-and-ci-ticket-10--2026-10-14)) | **Every cell of the real install/upgrade/rollback/uninstall matrix** (systemd and launchd, system and user mode, default and custom CA, four platforms): requires disposable hosts and authorization |
| 5 | SSH and release hardening | In-process SSH server suite (host-key binding, progress grammar, timeouts, secrecy); four artifacts built, signed, assembled and verified with a throwaway key; headers, limits, shutdown, backup tests ([Ticket 09](#operational-hardening-ticket-09--2026-10-14)); `actionlint` clean workflows | Real `sshd` behavior; GitHub workflows have never run (runner labels, SHA pinning, `release` environment, signing secret); a production signing key does not exist; real reverse proxy; `rsa` advisory RUSTSEC-2023-0071 open |

Documentation and benchmark deliverables of Ticket 11 are complete; they add no stage verification beyond stage 3's macOS measurement. Licensing: no `LICENSE` file exists and the owner has not decided one (see README).
