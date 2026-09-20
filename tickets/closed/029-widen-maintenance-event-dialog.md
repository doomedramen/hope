---
id: "029"
title: "Widen the Create maintenance event dialog"
status: closed
priority: medium
created: "2026-09-20T20:00:00Z"
updated: "2026-09-20T22:52:04Z"
tags: ["maintenance", "dialog", "layout", "ux"]
---

## Problem

The "Create maintenance event" dialog is way too narrow. Widen the dialog so its content remains usable.

## Context

This request follows the earlier maintenance dialog work; the dialog still appears too narrow in the current application.

## Resolution

- Set the dialog width to `min(90vw, 80rem)` at the desktop breakpoint while keeping the existing mobile gutter.
- Added a Playwright regression that opens the dialog at 1440×900, 1024×768, and 390×844, verifies both actions remain available, and rejects document-level horizontal overflow.
- Manually verified the dialog and footer at 1440×900, 1024×768, 768×1024, and 390×844 in the running application.

## Verification

- `pnpm -C apps/web test -- --run src/components/maintenance/MaintenanceEventForm.test.tsx`
- `E2E_AGENT_TARGET_SKIP_BUILD=1 pnpm -C apps/web exec playwright test e2e/maintenance.spec.ts --workers=1`
- `pnpm -C apps/web build`
- `pnpm -C apps/web lint` (existing warnings only)
