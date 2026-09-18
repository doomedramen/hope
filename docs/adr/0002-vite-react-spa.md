# 0002. Vite + React + TypeScript SPA, served statically by axum

## Status
Accepted

## Date
2026-09-18

## Context
Spec §5.2 recommends a React/TypeScript frontend and lists the web UI as either bundled with the server or served as a static frontend. §13 describes an authenticated, single-user-oriented operator UI (device pages, topology, maintenance calendar) with no public/anonymous content and no SEO surface.

## Decision
Build the frontend as a Vite + React + TypeScript single-page application, built to static assets and served directly by the axum server alongside the JSON API.

## Alternatives considered
- **Next.js** — rejected. The UI sits entirely behind login with no SSR or SEO requirement, so server rendering buys nothing. Next's static export mode fights dynamic routes like `/devices/[uuid]`, and running Next's own Node server would split authentication and CSRF handling across two runtimes (Node for the UI, axum for the API) instead of one.
- **Server-rendered templates (Askama/Tera in axum)** — rejected: the UI is data-heavy and interactive (topology graph, live tables), which is a poor fit for server-rendered HTML and would require a separate API layer anyway.

## Consequences
A single Rust process owns sessions, CSRF, and authorization for both the API and the UI's static assets, simplifying the auth boundary described in §12.3. The frontend has no server-side rendering; all data fetching happens client-side against `/api/v1`. Deploys are simpler (one container serves everything per §5.2's Docker Compose packaging).
