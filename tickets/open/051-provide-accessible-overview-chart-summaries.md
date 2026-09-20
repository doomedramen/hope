---
id: "051"
title: "Provide accessible summaries for Overview chart data"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["overview", "charts", "accessibility", "data-visualization"]
---

## Problem

The Service health and Infrastructure signals charts expose visual axes and
hover/touch tooltips but no chart-specific accessible name, text summary, or
data-table alternative. The surrounding cards describe the topic but not the
chart's values, range, or main change.

Screen-reader and keyboard users cannot obtain the same trend information from
the Overview charts without reconstructing it from graphical elements.

## Context

The charts use Recharts `accessibilityLayer`, but the shared ChartContainer has
no accessible summary or data alternative. The UX review's accessibility tree
showed tick labels without a chart-level description.
