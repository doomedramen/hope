---
id: "028"
title: "Improve maintenance recurrence UI"
status: closed
priority: medium
created: "2026-09-20T18:01:00Z"
updated: "2026-09-20T19:53:40Z"
tags: ["maintenance", "recurrence", "usability", "feature-request"]
---

## Problem

The maintenance event form exposes recurrence as a single free-form “Recurrence rule” text field. Users must know RRULE syntax to create recurring maintenance events. Provide a more approachable recurrence UI while retaining an advanced option for raw RRULE input.

## Context

The hosted Create maintenance event dialog only shows the placeholder `FREQ=WEEKLY;BYDAY=SA` and text that RRULE content is optional. I successfully used `FREQ=DAILY;COUNT=3`, but the form gives no guided controls for frequency, interval, weekdays, or end condition. Ticket 024 covers malformed-rule validation; this ticket covers the recurrence configuration experience.
