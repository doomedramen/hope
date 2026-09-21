# 0020. Standard-port discovery by default

## Status

Accepted; supersedes the complete-port default in ADR-0014.

## Context

A full TCP scan probes 65,535 ports on every approved address. With two
connections per host and a one-second timeout, one inactive host can occupy the
worker for roughly nine hours. Progress then appears stuck. Routine discovery
needs a bounded, predictable plan. Operators still need an explicit full scan
for unusual services.

## Decision

Initial discovery and scheduled change scans probe a fixed, ordered set of 39
common TCP ports. Routine connect attempts use the smaller of the scope timeout
and 250 ms. An explicitly requested device `full_tcp` run checks all 65,535
ports on one current device address and retains the scope timeout. Both modes
use the same approved scope, stored concurrency limits, cancellation, durable
progress, and classification path.
The device and its address are checked again before the worker probes them.

The run's planned probe count and target identity identify its port plan. This
preserves the full plan for routine jobs queued before this change and keeps
retry offsets stable.
Closure evidence requires an explicit `closed` observation for the same port.
Unscanned and filtered ports cannot close prior open-port evidence.

The UI shows the standard probe count before confirmation. Once a scope is
confirmed, standard scan starts directly. A device detail page offers a manual
full scan with an additional review. Primary status shows scan type and
progress; technical job details and audit events stay in a collapsed section.

## Consequences

Routine scans can miss services on unlisted ports or slow connections. Use an
explicit full scan when exhaustive discovery is needed. A completed standard
scan is authoritative only for its scanned ports. The curated list can change
for new runs, but a list change must preserve queued run plans and retry order.
