---
id: "007"
title: "Humanize selected change category"
status: open
priority: medium
created: "2026-09-20T17:11:27Z"
updated: "2026-09-20T17:11:27Z"
tags: ["changes", "filters", "ux"]
---

## Problem

The Changes category filter displays the internal snake-case value after selection instead of the human-readable category label.

## Context

Steps observed:

1. Open Changes.
2. Select a category such as `Monitor.Created`.

Actual result: the option list uses `Monitor.Created`, but the closed filter trigger displays `monitor.created`.

Expected result: keep the selected trigger label consistent with the option label, for example `Monitor.Created`.
