# 0008. WebSocket over mTLS for agent transport

## Status
Accepted

## Date
2026-09-18

## Context
Spec §7.1 requires the agent to initiate its own outbound connection and operate correctly on LAN-only hosts with no internet access. §14.2 defines an agent protocol with hello/capabilities, heartbeat, inventory snapshots, observations, and command request/ack/result — a persistent, bidirectional, low-latency channel — versioned independently of the agent binary, with every message carrying a unique ID for idempotent retries.

## Decision
Use a single outbound WebSocket connection per agent, carried over mTLS (ADR-0007), via `tokio-tungstenite`. Frames are JSON payloads compressed with zstd, each carrying a unique message ID, defined in a versioned protocol crate shared between agent and server (ADR-0001).

## Alternatives considered
- **gRPC (bidirectional streaming)** — rejected. gRPC's HTTP/2 stack adds complexity for an agent that must build cleanly as a static musl binary and traverse ordinary outbound firewalls; a single long-lived WebSocket is simpler to reason about for reconnect/backoff behaviour and easier to proxy through typical homelab network setups.
- **HTTPS polling** — rejected: command delivery (§7.6 server-triggered update, §14.2 command request/ack) needs near-real-time push to the agent, which polling only approximates at the cost of either high latency or high request volume at scale (§5.3, up to 500 agents).

## Consequences
The agent needs reconnect-with-backoff logic and must buffer bounded observations while disconnected (§7.1). The shared protocol crate versions independently of the agent binary (§14.2), and the server must tolerate agents a defined number of protocol versions behind. zstd framing keeps inventory snapshots compact for the buffered/disconnected case.
