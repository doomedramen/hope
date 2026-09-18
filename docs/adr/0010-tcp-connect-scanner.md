# 0010. TCP connect scanning only, SYN scanning deferred behind a trait

## Status
Accepted

## Date
2026-09-18

## Context
Spec §6.3 requires full-port (1–65535) TCP scanning for initial discovery and periodic change scans, with configurable connect-or-SYN scanning depending on privileges, bounded concurrency, jitter/backoff, and a policy to exclude fragile devices. §6.3 also forbids login attempts or exploit probes — scanning must stay a low-risk reachability check.

## Decision
Implement TCP connect scanning as the only scan strategy for v1, with semaphores bounding global, per-network, and per-host concurrency. Define a `Scanner` trait so SYN scanning can be added later as an alternate implementation without changing callers.

## Alternatives considered
- **SYN scanning (raw sockets) from the start** — rejected for v1. It requires elevated capabilities (`CAP_NET_RAW`) that complicate the container/deployment story and increase the privilege footprint (§7.4's "minimise risk" principle applies equally to the scanner), for a speed benefit that matters less at homelab scale (§5.3: 1–10 subnets, up to 1,000 devices) than at internet scan scale.
- **Wrapping `nmap` as a subprocess** — rejected: adds an external binary dependency, complicates the static-binary/container packaging goal (§5.2), and makes bounded concurrency, jitter, and partial-scan tracking (§6.3) harder to control precisely than a native async scanner.

## Consequences
Full-port scans are slower than SYN scanning would allow, which is acceptable at the target scale. The `Scanner` trait boundary means adding SYN scanning later (spec's stated "SYN scanning later behind a trait") is additive, not a rewrite, but it will still need a plan for the elevated-privilege deployment story when it lands.
