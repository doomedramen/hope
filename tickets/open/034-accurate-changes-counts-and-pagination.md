---
id: "034"
title: "Add accurate Changes counts and pagination"
status: open
priority: high
created: "2026-09-20T22:16:00Z"
updated: "2026-09-20T22:16:00Z"
tags: ["changes", "counts", "pagination", "api", "dashboard"]
---

## Problem

The "Changes" page shows inaccurate numbers because its API queries are
limited to 100 items. Add a count endpoint that returns the real totals and
add pagination to list responses.

## Context

There may be similar cases elsewhere in the application where dashboard
numbers or lists are based on a capped API response; inspect the other list
consumers for the same issue.
