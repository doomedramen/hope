# ADR-0018: dependency-aware alerting

## Status

Accepted for Milestone 8.

## Context

Inventory already stores containment and manually authored dependency edges,
but monitor incidents are independent. A failed host can therefore produce a
notification for the host and every monitored service below it. Suggested
inferences also need an explicit confirmation boundary before they affect
operations.

## Decision

PostgreSQL remains authoritative for dependency topology and notification
suppression state. Dependency edits use typed graph handlers that validate
self-cycles and indirect cycles under a bounded graph walk. Confirmed edges
participate in operational traversal; suggested and rejected edges remain
visible in the API but do not affect alerting. The graph API exposes a bounded
blast-radius preview and explicit confirm/reject actions.

Containment edges and service ownership reconcile into deterministic inferred
edges. Reconciliation is idempotent. When an inferred edge is no longer
backed by inventory, it is removed unless a suppression record references it;
referenced stale edges are retained as rejected historical evidence.

When a monitored service opens an incident, the scheduler finds bounded paths
to confirmed upstream dependencies with an open monitored incident. Hard
dependencies, or edges with health propagation enabled, suppress the child
notification. The child incident and monitor observations are still written.
Each suppression stores the root incident, dependency edge, full path, target
entity, event, and a technical reason. Incident list/detail responses include
these records.

## Alternatives considered

- **Suppress the child incident itself** — rejected; incident history and
  independent recovery state must remain available.
- **Keep suppression state only in memory** — rejected; restarts and multiple
  workers must not lose the explanation for a missing notification.
- **Treat every suggested inference as active** — rejected; non-deterministic
  topology requires operator confirmation.

## Consequences

- Parent failures produce one actionable notification while downstream
  evidence remains queryable.
- A child still alerts when no upstream incident is open.
- Stale topology cannot silently continue suppressing alerts.
- The frontend can render graph previews and suppression reasons from the API;
  frontend screens are intentionally deferred to the next UI work session.
