---
id: "031"
title: "Expose network scanner job state and logs"
status: closed
priority: high
created: "2026-09-20T20:00:00Z"
updated: "2026-09-20T22:54:00Z"
tags: ["infrastructure", "networks", "discovery", "scanning", "jobs", "observability"]
---

## Problem

The network scanner/discovery needs to be fleshed out a lot more. Operators should be able to see if an existing job is running and should not be able to start another one while it is running. Operators should also be able to see logs and other job details.

## Resolution

- Added persisted scan-run and job snapshots with polling for active work.
- Enforced one pending/running scan per network in the API and database, with a conflict response for duplicate launches.
- Added cancellation, worker progress, attempts, timestamps, terminal state, recent history, and audit activity logs to the Networks UI.

## Verification

- `pnpm -C apps/web test -- --run src/components/DiscoveryScopeSetup.test.tsx`
- `E2E_AGENT_TARGET_SKIP_BUILD=1 pnpm -C apps/web exec playwright test e2e/infrastructure.spec.ts --workers=1`
