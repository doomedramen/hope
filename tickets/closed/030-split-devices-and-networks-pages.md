---
id: "030"
title: "Split Devices and Networks into separate pages"
status: closed
priority: high
created: "2026-09-20T20:00:00Z"
updated: "2026-09-21T08:31:00Z"
tags: ["infrastructure", "devices", "networks", "navigation", "ux"]
---

## Problem

The "Devices and network records" page needs a big redesign. The "Networks" section is huge and the "Devices" section is easy to miss. Make them two separate pages.

## Resolution

- Added dedicated `/devices` and `/networks` routes with separate navigation entries.
- Kept device inventory/search on Devices and network management/discovery on Networks.
- Removed secondary cross-links from each page so Devices and Networks stay focused; primary navigation remains the way to switch areas.

## Verification

- `E2E_AGENT_TARGET_SKIP_BUILD=1 pnpm -C apps/web exec playwright test e2e/infrastructure.spec.ts --workers=1`
- `pnpm -C apps/web build`
- `pnpm -C apps/web exec prettier --check src/routes/infrastructure.tsx src/routes/networks.tsx e2e/infrastructure.spec.ts`
