---
id: "036"
title: "Hide backend error details from the operator UI"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["ux", "error-handling", "accessibility"]
---

## Problem

Several UI error states render the raw `Error.message` returned by API calls.
On the Overview page, the error banner exposed `error returned from database:
syntax error at or near "="`. The same presentation pattern appears in
Changes, Settings, Monitoring, Agents, discovery, maintenance, and
authentication flows.

These implementation-level messages do not tell an operator what is available,
what failed, or what recovery action to take.

## Context

The Overview page still rendered partial dashboard data and a Retry control
while showing the database syntax error. Ticket 001 covered a prior dashboard
database failure; this ticket concerns the UI's system-wide treatment of any
backend or transport error.
