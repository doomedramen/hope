---
id: "044"
title: "Add asset context and navigation to change events"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["changes", "navigation", "observability", "ux"]
---

## Problem

The Changes feed repeatedly labels records as "Monitors record changed" and
identifies them with short opaque IDs such as `f600eb55`. Categories such as
`Incident.Opened` remain internal-style labels, and monitor, service, and
proposal changes have no visible navigation path to the affected record.

At phone width, a feed of critical changes becomes a sequence of visually
similar entries that does not identify the device, service, endpoint, or
incident that changed.

## Context

Device changes have a limited "Open inventory" link. Ticket 027 records a
previous issue with that device path. This finding covers the missing context
and navigation for non-device change entities, which dominate the observed
feed.
