# Milestone 2 design: approved network discovery

Spec refs: §5.3, §6, §11, §14.1, §15.1, §16, §17 M2. This document
defines the safety and persistence rules for network discovery. M1 owns the
canonical device/evidence model; M2 writes observations into it without
changing confirmed operator facts.

## 1. Scan scope is the hard gate

`networks` remains the location model. A one-to-one `discovery_scopes` record
holds the operational permission to scan that network.

1. An operator creates a scope draft with exclusions and a scan profile.
2. The server parses the network CIDR and calculates the exact scan target
   count after exclusions.
3. The operator sends that same count to the confirm route.
4. Only a scope with `confirmed_at` and a matching
   `confirmed_target_count` can enqueue discovery or full-TCP jobs.

Changing exclusions or profile clears confirmation. The first M2 slice accepts
only private RFC1918 IPv4 scopes up to 65,534 scanable hosts. IPv6 discovery
and a deliberate broad-range override need their own operator workflow.

`discovery_scopes` also stores defaults the planner will use: normal or
low-impact profile, global worker policy inputs, per-host concurrency, connect
timeout, and discovery/full-TCP cadence. The worker treats these as limits, not
suggestions.

### 1.1 Low-impact policy

The worker resolves `scan_profile` when it loads a run. `normal` preserves the
stored TCP global, scope, and per-host limits. `low_impact` applies reductions
only: global and scope concurrency are capped at 8, per-host concurrency at 1,
and classification fan-out at 2. Classification fan-out is also bounded by the
stored TCP and per-host limits, so policy resolution never increases an
operator limit.

Both profiles keep the complete TCP plan (`1–65535` for every approved
target). Low-impact work adds a shared per-run start interval, deterministic
per-probe jitter, and a capped batch backoff. TCP and classification phases use
separate pacing state. The run UUID seeds the schedule, so concurrent runs do
not begin with one synchronized delay pattern, while retries of one run remain
reproducible. Clock, jitter, and sleeper seams make policy tests deterministic.

Pacing waits are cancellation-aware and run outside scanner semaphores. The
existing cancellation monitor and 60-second lease heartbeat continue during
long low-impact waits. A cancelled or lease-lost run stops before launching
the next probe; it never turns pacing into skipped successful work.

## 2. Scan runs and partial results

The next migration adds `scan_runs` and `port_observations`.

```text
scan_runs(id, network_id, kind, status, requested_by, started_at, finished_at,
          targets_planned, targets_completed, ports_planned, ports_completed,
          source, error, cancellation_requested, created_at)

port_observations(id, scan_run_id, device_id nullable, address, port,
                  transport, state, observed_at, latency_ms, evidence_id nullable)
```

Run status is `pending`, `running`, `succeeded`, `failed`, or `cancelled`.
`complete` means every planned target and port completed. A failed or cancelled
run remains partial and can add open-port evidence, but it cannot close an
existing port because unvisited ports have no absence evidence.

TCP uses `open`, `closed`, or `filtered` only where the connect result supports
that conclusion. M2 targeted UDP uses `open`, `closed`, and
`open_or_filtered`; silence never becomes `closed`.

## 3. Worker and scanner modules

`discovery::tcp::Scanner` is the seam used by TCP scan jobs. M2 supplies one
adapter: `ConnectScanner`, backed by `tokio::net::TcpStream::connect` with
timeouts. SYN scanning stays out of v1 under ADR-0010.

`discovery::udp::Scanner` is the seam used by targeted UDP jobs. M2 supplies
`UdpScanner`, backed by an ephemeral `tokio::net::UdpSocket`. Each call takes
an explicit port and configured safe payload; no API expands a target into a
full UDP port range. Global, network, and per-host semaphores bound work, and
the scanner applies a bounded inter-probe interval. A response is `open`, an
ICMP port-unreachable result is `closed`, and timeout or other silence is
`open_or_filtered`.

`udp::persist_observations` writes these states to the existing
`port_observations` table with `transport = 'udp'`. It does not update run
progress or infer closure, so TCP worker observation application remains
unchanged and partial UDP work cannot create absence evidence.

The scan coordinator owns the semaphores in this order: global, scope, then
host. It checks cancellation before each target and port, heartbeats the job
lease while a run is active, and records completed work in `jobs.progress`.
Open-port classification uses a separate policy-resolved concurrency bound;
normal runs cap at eight and low-impact runs cap at two, subject to stored TCP
and per-host limits. Each classifier retains its own connection and deadline
limits, so a large open-port batch cannot create unbounded re-probing. A
cancellation signal aborts in-flight classifier probes and uses the generic
`tcp` fallback for open observations that still need persistence. Classification
results are reordered to observation order before persistence, so service
reconciliation remains in the same transaction and ordering as the raw port
observations.
The worker polls cancellation every 100 ms and renews the 60-second job lease
every 10 seconds with a five-second heartbeat timeout, including progress
counts while classification is active.
The scanner never performs authentication, protocol writes, or exploit probes.

## 4. Observation application

The worker records every TCP result for every approved address. `open` and
`closed` (connection refused) prove address liveness. Only after the first
definitive result for a previously unresolved address does the worker resolve
an owner or create an unconfirmed `unknown` device with one current M1
interface/address record. A new address whose complete result set is only
`filtered` or unresponsive keeps raw `port_observations` with a nullable
`device_id`; it creates no device, interface, address, or device evidence.
Previously known address owners may keep their existing association, but a
filtered-only result does not refresh or discover an owner. If filtered
observations were written before a later port proves liveness, the worker
attaches those observations to the resolved device in the same run.

An open TCP port appends a `network_scan` evidence row with attribute
`open_port` and value `{"address":"...","port":443,"transport":"tcp"}`.
It emits a `port.opened` change event only when the latest matching scan
evidence was absent or missing; repeated positive observations refresh evidence
without duplicating the event.

A full, complete scan can append absent evidence and a `port.closed` change
event for a previously observed port only when the historical evidence belongs
to the same device observed at that address by the run, after resolving
`canonical_of` redirects. Closure reconciliation considers only addresses with
at least one definitive `open` or `closed` result, so a filtered-only or
unresponsive address cannot close historical open-port evidence. If the address
has a different or ambiguous owner, the run retains its raw observation but
writes no absence evidence or closure event for the prior owner. Closure
reconciliation runs in the same transaction that marks the run `succeeded`.
Partial, failed, and cancelled runs only retain raw observations and refresh
positive evidence; they never write absence evidence or closure events.

## 4.1 Basic open-port classification

M2 classifies every open TCP observation that has a resolved device. It writes
the result into the M1 `services` and `endpoints` tables; it does not create a
fingerprinting table or a parallel port-service model.

The classifier has a per-port connection budget, connect/read timeouts, an
overall deadline, bounded banner/header/body samples, and a small redirect
limit. It sends no credentials, cookies, authorization headers, login
attempts, or exploit payloads. Its only application write is one ordinary
HTTP `GET` request with `Connection: close`; SSH classification stops at the
server banner. A banner beginning with `SSH-` produces `ssh` evidence.

HTTP responses produce `http` evidence containing status, an allowlisted
header subset, bounded title/body samples, and same-endpoint redirects. A TLS
handshake followed by an HTTP response produces `https`; a TLS handshake with
no HTTP response produces `tls`. TLS certificate metadata is limited to
fingerprint, subject/SAN, issuer, validity, and negotiated ALPN. The
observation verifier lets the handshake finish so metadata can be collected,
but deliberately does not establish trust or make credentials available;
evidence records `certificate_trust = not_validated`.

If no protocol responds, the open port still produces `tcp` classification and
remains visible. The result is stored as `network_scan` evidence on the
canonical service with attribute `protocol_classification`. One canonical
`socket` endpoint is reused for each device/address/port. Repeated scans
refresh evidence and endpoint timestamps without duplicating services or
change events. An automatic classification may replace an earlier automatic
classification when the protocol changes, but a confirmed manual protocol is
not overwritten. Complete closure reconciliation marks the matching socket
endpoint non-current; partial runs never do so.

M3 adds product signatures, versions, mDNS/DNS-SD, UPnP, and richer
reconciliation. Those are outside this M2 classifier.

## 5. Interfaces

The scope interface is session-authenticated and CSRF-protected:

```text
POST /api/v1/networks/{network_id}/discovery-scope
  { "excluded_cidrs": ["192.168.10.10/32"], "scan_profile": "low_impact" }

POST /api/v1/networks/{network_id}/discovery-scope/confirm
  { "target_count": 252 }
```

Both actions write audit events. A later `POST /api/v1/networks/{id}/scans`
requires an idempotency key and a confirmed, enabled scope; it creates a job and
returns its stable job UUID.

## 6. Recurring change scans

The server scheduler re-plans due change scans every five minutes and on every
server start. It reads `full_tcp_interval_seconds` from each scope. A scope is
eligible only when it is enabled, confirmed, and its confirmed target count
still matches `target_count`.

The planner skips a scope with a pending/running scan or any scan created
within its full-TCP cadence. This prevents an operator-triggered initial/full
scan and a recurring change scan from overlapping. The job idempotency key is
`discovery.change_scan:{network_id}:{cadence_window}`. The planner creates the
`discovery.full_tcp` job and its `scan_runs` row in one transaction, with
`kind = change_scan`, `source = scheduler`, and the current scope version.
PostgreSQL job idempotency plus the unique `scan_runs.job_id` relation makes
concurrent scheduler instances safe.

M2 has no persisted quiet-period or maintenance subsystem yet. The planner
does not invent a bypass; future quiet/maintenance gates must be checked before
the same transactional enqueue step.

Low-impact pacing is not a maintenance substitute. It does not pause during a
quiet period, inspect maintenance events, or change job eligibility. Those
gates belong to the later maintenance milestone and must be added at the
transactional planning boundary.

## 7. Test seams

- `domain::discovery::ApprovedScope::parse`: private-range, size, exclusion,
  and exact target-count policy.
- Scope draft/confirm routes: confirmation resets on change; mismatched count
  cannot confirm.
- `discovery::Scanner`: local open/closed TCP ports and timeout handling.
- `discovery::udp::Scanner`: local UDP reply plus injected closed and
  ambiguous transport outcomes; explicit probe-list and concurrency bounds.
- Run application: partial runs cannot close ports; complete runs emit accurate
  open/closed change events.
- Scan policy: low-impact resolution only reduces stored limits; complete
  port plans remain unchanged; injected clock/jitter/sleeper seams verify
  deterministic bounded pacing.
- Basic classifier: local HTTP, TLS/HTTPS, SSH-banner, generic TCP, bounded
  redirects/body, and sensitive-header fixtures.
