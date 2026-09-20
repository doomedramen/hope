---
id: "042"
title: "Make monitor and incident lists actionable for triage"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["monitoring", "incidents", "triage", "ux"]
---

## Problem

The Monitors page showed 21 rows with 16 Down states interleaved with healthy
rows, but offers no visible severity or state filter, ordering control, row
detail, or navigation to the affected asset. The Incidents page similarly
showed many open Critical incidents as inert rows.

Several rows repeat the same target while exposing only short service and
endpoint identifiers. An operator cannot use either list to move from a
failure to the affected asset, monitor, cause, or next action.

## Context

The monitoring and incident table rows are rendered as presentation rows with
no row-level link, button, or detail action. On the observed data set, repeated
HTTP and HTTPS failures made the absence of prioritization and a drill-down
path especially pronounced.
