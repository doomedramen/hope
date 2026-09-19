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
that conclusion. The later UDP path uses `open`, `closed`, and
`open_or_filtered`.

## 3. Worker and scanner modules

`discovery::Scanner` is the seam used by scan jobs. M2 supplies one adapter:
`ConnectScanner`, backed by `tokio::net::TcpStream::connect` with timeouts.
SYN scanning stays out of v1 under ADR-0010.

The scan coordinator owns the semaphores in this order: global, scope, then
host. It checks cancellation before each target and port, heartbeats the job
lease while a run is active, and records completed work in `jobs.progress`.
The scanner never performs authentication, protocol writes, or exploit probes.

## 4. Observation application

For each address, the worker resolves or creates an unconfirmed device through
M1 identity reconciliation. An open TCP port creates a `network_scan` evidence
row and a `port.opened` change event when it was not already current. A full,
complete scan can add absent evidence and a `port.closed` change event for a
previously observed port. Partial scans only refresh positive evidence.

M3 consumes the open-port evidence for HTTP, TLS, SSH, and generic TCP
classification. M2 does not create product or service guesses.

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

## 6. Test seams

- `domain::discovery::ApprovedScope::parse`: private-range, size, exclusion,
  and exact target-count policy.
- Scope draft/confirm routes: confirmation resets on change; mismatched count
  cannot confirm.
- `discovery::Scanner`: local open/closed TCP ports and timeout handling.
- Run application: partial runs cannot close ports; complete runs emit accurate
  open/closed change events.
