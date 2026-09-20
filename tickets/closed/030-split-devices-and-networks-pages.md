---
id: "030"
title: "Split Devices and Networks into separate pages"
status: closed
priority: high
created: "2026-09-20T20:00:00Z"
updated: "2026-09-20T22:53:03Z"
tags: ["infrastructure", "devices", "networks", "navigation", "ux"]
---

## Problem

The "Devices and network records" page needs a big redesign. The "Networks" section is huge and the "Devices" section is easy to miss. Make them two separate pages.

## Resolution

- Added dedicated `/devices` and `/networks` routes with separate navigation entries.
- Kept device inventory/search on Devices and network management/discovery on Networks.
- Added cross-links so operators can move between the two related areas without returning to the dashboard.

## Verification

- `E2E_AGENT_TARGET_SKIP_BUILD=1 pnpm -C apps/web exec playwright test e2e/infrastructure.spec.ts --workers=1`
