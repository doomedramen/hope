# 0004. PostgreSQL as the authoritative database

## Status
Accepted

## Date
2026-09-18

## Context
Spec §5.1 requires a modular monolith with background workers (not microservices) sharing one authoritative store, and explicitly avoids mandatory Redis, Kafka, Elasticsearch, or Kubernetes. §5.2 recommends PostgreSQL. The system needs a PostgreSQL-backed job queue with leases/retries/idempotency (§5.1), network/CIDR modelling for discovery (§6), exclusion constraints for maintenance windows (§10.3 conflict rules), and live UI updates (§5.2, §13).

## Decision
Use PostgreSQL as the single authoritative store for configuration, inventory, evidence, state, and bounded history. Keep hot in-memory state — current check state, agent sessions, the in-process dependency graph via `petgraph` — as a rebuildable cache only, never as the source of truth.

## Alternatives considered
- **SQLite** — rejected: single-writer semantics don't suit a server + worker(s) writing concurrently, and it has no network access story for a worker or agent gateway running as a separate process/host later. Postgres's `FOR UPDATE SKIP LOCKED`, `LISTEN/NOTIFY`, `inet`/`cidr` types, and `tstzrange` exclusion constraints are used directly by later decisions (ADR-0006, ADR-0009) and have no equivalent in SQLite.
- **In-memory-only state (no durable store)** — rejected outright; §12.5 and §11 require durable audit and change history, and a restart must not lose configuration or evidence.

## Consequences
All durable state lives in one place, simplifying backup (§15.2) and restore. In-memory caches (scheduler due-times, live graph) must be derived from Postgres on startup and treated as disposable. Splitting workers onto separate hosts later remains possible because Postgres, unlike SQLite, is already network-accessible.
