# ADR-0019: maintenance planning and reservations

## Status

Accepted for Milestone 9.

## Context

Disruptive work needs a durable schedule that can be checked against
inventory dependencies before an operator commits it. A one-time event and
each future recurrence must reserve the same resources, including lead-in and
cooldown time. During the reserved window, expected monitor failures should
remain visible without creating duplicate downstream notifications.

## Decision

PostgreSQL stores maintenance events, normalized resources, and expanded
occurrences. Events carry an IANA timezone, a one-time or RFC 5545 recurrence
rule, a bounded 90-day expansion horizon, lead-in/cooldown, expected-failure
policy, and an optimistic version. Jiff performs application time arithmetic;
the `rrule` dependency is isolated to recurrence parsing and expansion.

Creation and edits take a transaction-scoped advisory lock, expand all
future occurrences, and compare their reservation windows with other active
plans. Conflicts include shared exclusive resources, an affected/required
resource relationship, confirmed dependency paths, and the default global
disruptive-event lock. The API returns the relationship, overlap, and a
non-destructive later-start suggestion; conflicting edits roll back.

The reconcile job advances events through scheduled, upcoming, active,
overrunning, and completed states. A past planned end with an open expected
incident becomes overrunning and retains its reservation until the operator
completes or cancels the event. Overruns generate idempotent notifications via
the existing configured webhook/ntfy channels. Expected incident openings
retain a maintenance suppression record tied to the exact event and
occurrence; incident observations and recovery remain authoritative.

## Consequences

- Operators can inspect future conflict reasons without mutating either plan.
- DST-sensitive recurring schedules are resolved in the event timezone and
  reject ambiguous or nonexistent local times.
- Reservation state survives process restarts and is safe to reconcile more
  than once.
- The API is complete before the calendar/timeline frontend, which remains a
  separate UI work session.
