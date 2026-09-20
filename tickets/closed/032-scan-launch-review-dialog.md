---
id: "032"
title: "Require a review dialog before starting scans"
status: closed
priority: medium
created: "2026-09-20T21:18:10Z"
updated: "2026-09-20T22:54:23Z"
tags: ["infrastructure", "networks", "discovery", "scanning", "dialog", "ux"]
---

## Problem

Triggering a scan should be via a dialog.

## Context

Scan launch actions currently submit directly from the network discovery controls; require an explicit dialog step before enqueueing a scan.

## Resolution

- Added a review dialog that summarizes the network and confirmed target count before enqueueing work.
- Require an explicit acknowledgement of the full TCP sweep before enabling the launch action.
- Reset the acknowledgement each time the dialog opens and keep the underlying launch control separate from submission.

## Verification

- `pnpm -C apps/web test -- --run src/components/DiscoveryScopeSetup.test.tsx`
- `E2E_AGENT_TARGET_SKIP_BUILD=1 pnpm -C apps/web exec playwright test e2e/infrastructure.spec.ts --workers=1`
