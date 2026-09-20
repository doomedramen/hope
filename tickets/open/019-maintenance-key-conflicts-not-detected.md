---
id: "019"
title: "Detect maintenance conflicts for resource keys"
status: open
priority: high
created: "2026-09-20T17:42:48Z"
updated: "2026-09-20T17:42:48Z"
tags: ["maintenance", "conflicts", "resources", "data-integrity"]
---

## Problem

Overlapping maintenance events using the same resource key can both be accepted because the key is not retained as a reserved resource.

## Context

The existing QA maintenance event reserves `network-core` from 19:17–20:17. I created `QA conflicting event` with the same `network-core` key from 19:40–20:40. The second event was accepted, its resource rendered as `Unidentified resource`, and its Conflict check reported `No overlapping reservation found.` The resource key must survive persistence and participate in conflict detection.
