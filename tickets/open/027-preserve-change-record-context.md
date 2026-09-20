---
id: "027"
title: "Preserve record context from Changes inventory links"
status: open
priority: medium
created: "2026-09-20T17:56:25Z"
updated: "2026-09-20T17:56:25Z"
tags: ["changes", "infrastructure", "navigation", "usability"]
---

## Problem

The Changes page’s “Open inventory” link always navigates to the unfiltered Infrastructure page and does not select the record represented by the change. Operators lose context and must search for the affected device manually.

## Context

Filtering Changes to Notice → Device and opening the first event for `6fdac68c` navigated to `/infrastructure`, where the detail panel selected the default `catacomb` device instead of `QA test device` (`6fdac68c`). Include the record ID in navigation and select or focus that record on arrival.
