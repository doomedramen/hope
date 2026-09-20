---
id: "004"
title: "Clear device detail when search hides selected record"
status: closed
priority: medium
created: "2026-09-20T17:11:27Z"
updated: "2026-09-20T19:53:40Z"
tags: ["infrastructure", "search", "devices", "ux"]
---

## Problem

Filtering the device list can hide the selected device while its detail panel remains visible, creating a mismatch between the list and the detail view.

## Context

Steps observed:

1. Open Infrastructure with `QA test device` selected.
2. Enter `catacomb` in Search devices.

Actual result: the list shows only `catacomb`, but the detail panel below still shows `QA test device` and its Edit, Merge, and Split actions.

Expected result: clear the selection, select the first visible match, or otherwise indicate that the detail panel is outside the filtered result set.
