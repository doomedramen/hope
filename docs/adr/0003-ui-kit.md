# 0003. shadcn/ui + Tailwind, TanStack, React Flow + elkjs

## Status
Accepted

## Date
2026-09-18

## Context
Following ADR-0002, the SPA needs a component/styling system, data-fetching and table/routing primitives, and a way to render the dependency graph and topology views described in spec §9 (dependency graph) and §13.3 (device page explainability).

## Decision
Use shadcn/ui (Radix-based, copy-in components) with Tailwind CSS for styling; TanStack Router, TanStack Query, and TanStack Table for routing, server-state caching, and tabular views; React Flow with the elkjs layout engine for topology/dependency graph rendering.

## Alternatives considered
- **A full component framework (MUI, Ant Design, Chakra)** — rejected in favour of shadcn/ui's copy-in model, which keeps generated OpenAPI types and custom domain components consistent without fighting a themed component library.
- **Hand-rolled fetching/routing (React Router + custom hooks)** — rejected; TanStack Query's cache invalidation and TanStack Router's type-safe routes reduce boilerplate for the API surface in §14.1.
- **Cytoscape.js or vis-network for the graph** — rejected; React Flow integrates more naturally as React components for node/edge customisation, and elkjs gives layered layout quality suited to dependency trees.

## Consequences
The frontend gains a consistent, low-abstraction component base and typed API access when paired with `utoipa` + `openapi-typescript`/`openapi-fetch` (see README supporting library defaults). Team must maintain shadcn's copy-in components directly rather than upgrading via a package version bump.
