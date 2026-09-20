---
id: "023"
title: "Reject maintenance events with reversed times"
status: open
priority: high
created: "2026-09-20T17:53:26Z"
updated: "2026-09-20T17:53:26Z"
tags: ["maintenance", "validation", "data-integrity"]
---

## Problem

The maintenance event form accepts an End time earlier than Start and silently saves the event with the bounds swapped. This hides an operator mistake and changes the requested reservation window without acknowledgement.

## Context

I entered Start `2026-09-20T20:49` and End `2026-09-20T19:49` for `QA invalid times`; the event was created and displayed as `19:49–20:49`. Repeating the test with Start `21:30` and End `20:30` created `QA invalid times delayed` with the same normalization. Reject the submission inline with “End time must be later than start time” and preserve the entered values.
