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

- Discovery frame/cost measurements; completion targets passed in both controlled fixtures.
- Remaining report/retention boundary, restart, association, and 250-host retention load checks.
- Agent delivery fault tests and 30-minute RSS/CPU measurements.
- Expanded browser behavior and visible interactive review.
- Transactional installer harness, actual systemd/launchd service matrix.
- Isolated SSH provisioning integration tests.
- HTTPS/custom CA/proxy, persistent restart, SIGTERM shutdown, backup/restore.
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
