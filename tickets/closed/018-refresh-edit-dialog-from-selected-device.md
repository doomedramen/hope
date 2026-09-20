---
id: "018"
title: "Refresh edit dialog from selected device"
status: closed
priority: high
created: "2026-09-20T17:37:57Z"
updated: "2026-09-20T19:53:40Z"
tags: ["infrastructure", "devices", "dialogs", "data-integrity"]
---

## Problem

The Edit device dialog can show values from a previously selected device instead of the currently selected device.

## Context

I opened Edit for `QA VM device`, closed it, selected `QA test device`, and opened Edit again. The dialog displayed `QA VM device` in the Device name field even though the selected record was `QA test device`. Saving at that point could overwrite the wrong record with stale form values.
