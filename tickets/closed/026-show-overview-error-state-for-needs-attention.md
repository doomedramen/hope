---
id: "026"
title: "Show an error state for Overview needs-attention data"
status: closed
priority: medium
created: "2026-09-20T17:55:40Z"
updated: "2026-09-20T19:53:40Z"
tags: ["overview", "error-handling", "monitoring", "usability"]
---

## Problem

When a dashboard query fails, the Overview page shows the top-level error banner but leaves the Needs attention card in an indefinite “Loading dashboard data” skeleton. The card needs to resolve to an error or unavailable state so the page does not appear permanently busy.

## Context

With the Monitors request failing on the hosted app (`syntax error at or near "monitors"`), Overview displayed the expected “Some dashboard data is unavailable” banner, while the Needs attention section still exposed `status="Loading dashboard data"` after waiting more than a second. Show the failure and a retry path in that card, or explicitly explain that its data is unavailable.
