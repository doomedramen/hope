---
id: "045"
title: "Prioritize work surfaces over summary cards on phones"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["mobile", "information-architecture", "responsive", "ux"]
---

## Problem

At 375x812, summary cards stack into a long single-column sequence before the
page's primary work surface. Devices and Agents each present four cards before
their list, Changes presents three cards before the event feed, and Overview
presents four cards before operational detail.

On the empty Agents page, four zero-value metrics appear before the enrollment
list and action. On Devices, the inventory table begins well below the first
phone viewport.

## Context

This pattern appears across route-level `sm:grid-cols-*` summary sections that
become one column below the small breakpoint.
