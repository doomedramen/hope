---
id: "048"
title: "Confirm bulk monitor-proposal approval before execution"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["monitoring", "safety", "bulk-actions", "ux"]
---

## Problem

The "Approve all" action for pending monitor proposals immediately resolves
every proposal in the queue. Unlike rejection, it does not show a review or
confirmation surface, name the proposals that will be affected, or make the
scope visible before monitors are created.

An operator can inadvertently approve a changing queue of monitor proposals
with one compact action.

## Context

The bulk mutation iterates over all pending proposal items directly from the
header button. Individual rejection already uses a confirmation dialog.
