---
id: "038"
title: "Honor reduced-motion preferences in interface feedback"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["accessibility", "motion", "loading"]
---

## Problem

The web UI uses pulse, spin, and transition animations for connection status,
skeletons, spinners, dialogs, menus, and interactive cards, but no
`prefers-reduced-motion` or `motion-reduce` handling is present in the
application styles.

People who request reduced motion can still receive persistent loading and
interface motion without an alternative presentation.

## Context

The UI review found `animate-pulse`, `animate-spin`, and multiple transition
classes in shared components and route layouts, with no matching reduced-motion
media query or utility in `apps/web/src`.
