---
id: "043"
title: "Adapt monitoring and incident tables for narrow screens"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["monitoring", "incidents", "mobile", "responsive", "tables"]
---

## Problem

On a 375px viewport, the six-column Monitors table displays State and part of
Target while Check, Last result, Next run, and Failures fall off-screen. The
table can scroll horizontally, but the visible view does not make the overflow
or the most important triage values clear.

The incident table uses the same dense multi-column pattern. Important failure
information requires horizontal navigation during an outage.

## Context

The shared table wrapper allows horizontal scrolling. At the audited phone
width, its first screen showed a truncated "Che…" header and no on-screen
instruction or alternate compact representation.
