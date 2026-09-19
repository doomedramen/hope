# Architecture Decision Records

Decisions are recorded using the template in [`0000-template.md`](0000-template.md). Once accepted, an ADR is not edited for content changes — a later decision supersedes it and says so.

| ADR | Decision |
|---|---|
| [0001](0001-rust-backend.md) | Rust everywhere (server, worker, agent) in a single Cargo workspace. |
| [0002](0002-vite-react-spa.md) | Vite + React + TypeScript SPA, built static and served by axum; Next.js rejected. |
| [0003](0003-ui-kit.md) | shadcn/ui + Tailwind, TanStack Router/Query/Table, React Flow + elkjs for topology. |
| [0004](0004-postgresql-database.md) | PostgreSQL as the sole authoritative store; in-memory state is cache only. |
| [0005](0005-monitor-scheduling.md) | In-memory tokio scheduler for monitor checks, bypassing the job queue. |
| [0006](0006-custom-postgres-job-queue.md) | Custom Postgres-backed job queue (`SKIP LOCKED`, leases, idempotency keys). |
| [0007](0007-agent-mtls-auth.md) | mTLS with an internal CA for agent identity and enrollment. |
| [0008](0008-agent-websocket-transport.md) | WebSocket over mTLS for the agent-server protocol channel. |
| [0009](0009-sse-live-ui.md) | Server-Sent Events fed by Postgres `LISTEN/NOTIFY` for live UI updates. |
| [0010](0010-tcp-connect-scanner.md) | TCP connect scanning only for v1; SYN scanning deferred behind a trait. |
| [0011](0011-generic-webhook-ntfy-notifications.md) | Generic webhook + ntfy as the v1 notification providers. |
| [0012](0012-jiff-time-rrule-recurrence.md) | `jiff` for time maths; `rrule` for RFC 5545 recurrence, isolated from `chrono`. |
| [0013](0013-safe-basic-tcp-classification.md) | Bounded observation-only TCP protocol classification. |
| [0014](0014-low-impact-discovery-policy.md) | Explicit low-impact scan limits, deterministic pacing, and no M2 maintenance bypass. |
| [0015](0015-credential-vault.md) | External-key ChaCha20-Poly1305 credential vault with metadata-only API. |
| [0016](0016-ssh-agent-deployment.md) | Bounded, host-key-verified SSH install and repair jobs. |

## Supporting library defaults

These are working defaults, not individually debated ADRs — change any of them without ceremony if a better fit turns up during implementation.

- **Server:** `tokio`, `axum`, `tower-http`, `sqlx` (Postgres, checked queries, migrations), `figment`, `tracing` (JSON), `thiserror`/`anyhow`, `utoipa` (OpenAPI) feeding `openapi-typescript` + `openapi-fetch` on the frontend.
- **Auth:** `argon2`, `tower-sessions` (Postgres store), custom CSRF.
- **Probes:** `reqwest` (rustls), `tokio-rustls`, `x509-parser`, `hickory-resolver`, `surge-ping`, `mdns-sd`, `russh` (SSH install/repair).
- **Agent:** musl targets via `cargo-zigbuild`; `sysinfo`, `procfs`, `bollard`, `zbus`; bounded on-disk buffer (`redb`); `ed25519-dalek` + `sha2` for release verification.
- **Secrets:** AEAD (`chacha20poly1305`), master key from a mounted file, `secrecy` + `zeroize`.
- **Tooling:** `just`, pnpm workspace, `cargo-nextest`, `proptest`, `insta`, `testcontainers`, `cargo-deny`/`cargo-audit`, Playwright, GitHub Actions + buildx multi-arch.
