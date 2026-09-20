---
id: "040"
title: "Rework the multi-row primary navigation on phones"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["mobile", "navigation", "responsive", "ux"]
---

## Problem

At 375px wide, the seven or eight top-level destinations are rendered as a
two-column, four-row navigation block beneath the header. It consumes roughly
190px before the page title and primary task content begin.

On Overview, Monitoring, Agents, Changes, Networks, Maintenance, and Settings,
this makes operators scroll past navigation before reaching the page's purpose
or current condition.

## Context

The desktop sidebar correctly keeps navigation persistent. The phone layout
changes it to the multi-row grid in the root route, rather than providing a
compact mobile navigation pattern.
