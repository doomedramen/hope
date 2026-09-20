---
id: "041"
title: "Keep Monitoring view tabs usable at phone width"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["monitoring", "mobile", "tabs", "responsive"]
---

## Problem

The four Monitoring tabs do not fit on a 375px viewport. The first three tabs
occupy one row while "Monitor proposals" wraps alone onto a second row directly
against the Monitors card, making the selected-panel boundary difficult to
parse.

The wrapped label appears visually detached from the rest of the tab strip and
competes with the card header below it.

## Context

This was observed on the Monitors view at 375x812. Desktop and landscape
layouts keep all four tabs on one row.
