# 0009. Server-Sent Events driven by LISTEN/NOTIFY for live UI updates

## Status
Accepted

## Date
2026-09-18

## Context
Spec §5.2 allows "WebSocket or Server-Sent Events for live status" to the UI. The UI needs live updates for check state, incidents, and job progress (§8.3, §8.4, §14.1), but unlike the agent channel (ADR-0008) it is one-directional (server-to-browser) and doesn't need the browser to push structured commands back over the same channel — mutations go through the normal `/api/v1` HTTP API.

## Decision
Push live UI updates via Server-Sent Events, fed by PostgreSQL `LISTEN/NOTIFY` on relevant table changes (check state transitions, incidents, job progress).

## Alternatives considered
- **WebSocket for the UI** — rejected. The UI's live-update needs are one-directional; a WebSocket would require building reconnect, heartbeat, and multiplexing logic for a channel that never needs to carry client-initiated messages, when SSE gives automatic reconnection and simple HTTP semantics (works through ordinary proxies/load balancers) for free.
- **Client polling** — rejected: incidents and check-state changes need to appear promptly (§8.3, §8.4); polling at a useful interval wastes requests at the check volumes in §5.3 (up to 10,000 checks).

## Consequences
The UI's real-time channel is simpler than the agent's (no message framing, no bidirectional command protocol). `LISTEN/NOTIFY` payloads must stay small (Postgres's ~8KB notify payload limit), so notifications carry IDs/deltas and the client re-fetches full state via TanStack Query rather than receiving full objects over SSE.
