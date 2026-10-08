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
- Report/retention concurrency and recovery tests, 250-host retention load.
- Agent delivery fault tests and 30-minute RSS/CPU measurements.
- Expanded browser behavior and visible interactive review.
- Transactional installer harness, actual systemd/launchd service matrix.
- Isolated SSH provisioning integration tests.
- HTTPS/custom CA/proxy, persistent restart, SIGTERM shutdown, backup/restore.
- Signed artifacts for all four target platforms and protected release CI.

Never treat synthetic browser inventory as discovery evidence, command mocks as real service evidence, or cross-compilation as installation evidence.
