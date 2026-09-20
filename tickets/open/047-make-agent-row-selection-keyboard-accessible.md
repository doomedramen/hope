---
id: "047"
title: "Make agent row selection keyboard accessible"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["agents", "accessibility", "keyboard", "tables"]
---

## Problem

Agent rows are selected by an `onClick` handler on a table row. The rows expose
`aria-selected`, but they have no keyboard focus target, button/link semantics,
or keyboard activation handling.

Mouse users can select an agent and reveal its detail pane; keyboard users
cannot perform the same primary interaction from the list.

## Context

The affected interaction is in `apps/web/src/components/AgentsPage.tsx`.
The live environment had no enrolled agents, so the source review was used to
verify the row behavior.
