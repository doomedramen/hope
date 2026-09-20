---
id: "052"
title: "Move focus to main content after client-side navigation"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["accessibility", "navigation", "keyboard"]
---

## Problem

Client-side navigation changes the page content without moving keyboard or screen-reader focus to the new page context. The root layout makes the main region programmatically focusable, but does not focus it, or the new page heading, when a route changes. A keyboard user can activate a navigation item and remain at the previous control without an immediate indication that a different workspace has loaded.

## Context

The route shell in `apps/web/src/routes/__root.tsx` renders `main` with `tabIndex={-1}`, so a focus target already exists. Add intentional focus management after client-side navigation while preserving normal link and browser back/forward behavior.
