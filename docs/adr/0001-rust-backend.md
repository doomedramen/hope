# 0001. Rust everywhere for server, worker, and agent

## Status
Accepted

## Date
2026-09-18

## Context
Spec §5.2 recommends Rust for the agent (distributed as a single signed binary) and for the server (Axum), sharing protocol/domain crates with the agent where useful. The agent must run unprivileged-friendly, statically-linkable binaries on arbitrary Linux hosts (§7.1–7.2) with no runtime dependency footprint, and the server/worker/agent all need to share wire-format and domain types (§14.2 versioned protocol).

## Decision
Use Rust for the server, background worker, and agent, organised as a single Cargo workspace so domain, protocol, and shared logic crates are versioned and built together (spec §5.2 monorepo layout: `apps/{server,web,agent}`, `crates/*`).

## Alternatives considered
- **Go** — comparable static-binary story and simpler concurrency model, but weaker shared-type story with a TypeScript frontend (no natural `serde`/schema-export path), and less mature ecosystem for the memory-safety-sensitive agent privilege surface (§7.4).
- **Mixed stack (e.g. Go agent, different server language)** — rejected because it duplicates protocol types and drops the single-workspace atomic-change guarantee the spec calls for in §5.2.

## Consequences
One toolchain, one dependency-audit surface (`cargo-deny`/`cargo-audit`), and shared crates (`domain`, `protocol`) prevent drift between agent and server message formats. Requires the team to standardise on async Rust (tokio) across all components and accept Rust's steeper contribution curve for future collaborators.
