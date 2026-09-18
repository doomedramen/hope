# 0007. mTLS with an internal CA for agent authentication

## Status
Accepted

## Date
2026-09-18

## Context
Spec §7.3 requires a short-lived, single-use enrollment token exchanged for a durable agent identity, with the SSH credential used for installation never reaching the agent. §12.4 requires all agent-server traffic to use TLS, independently revocable per-agent identity, and enrollment tokens that expire quickly. §7.7 requires key rotation to be designed up front.

## Decision
Run an internal certificate authority (via `rcgen`) that issues short-lived client certificates. The one-time enrollment token is exchanged for an initial client cert; the agent renews its certificate automatically before expiry, and the server checks revocation status at every handshake. The agent gateway terminates TLS itself.

## Alternatives considered
- **Application-level Ed25519 keypairs (no TLS-layer identity)** — rejected. It would require building an equivalent authentication and revocation handshake at the application layer while still needing TLS in transport for §12.4, duplicating work that mTLS gives for free at the connection layer. mTLS ties transport security and identity together, which is a better match for §12.4's requirement that every agent has an independently revocable identity checked at connection time.
- **Static long-lived API tokens over TLS** — rejected: harder to rotate cleanly and does not naturally support the "renews automatically" and "revocation check at handshake" requirements of §7.3/§12.4.

## Consequences
Because agent identity is established at the TLS layer, the agent gateway **must terminate TLS itself** — it cannot sit behind a TLS-terminating reverse proxy (nginx/Caddy/Traefik in TLS-termination mode), since that would strip the client certificate before the application sees it. Any reverse proxy in front of the gateway must run in TCP passthrough mode. The internal CA's signing key becomes critical secret material: it must be included in the backup plan (§15.2) with documented recovery consequences if lost, since losing it strands every enrolled agent.
