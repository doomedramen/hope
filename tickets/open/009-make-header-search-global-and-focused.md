---
id: "009"
title: "Make header search global and focus it on entry"
status: open
priority: medium
created: "2026-09-20T17:11:27Z"
updated: "2026-09-20T17:11:27Z"
tags: ["search", "navigation", "ux"]
---

## Problem

The header control is labeled `Search devices, services, or networks`, but it only navigates to Infrastructure and does not focus the device search field.

## Context

Steps observed:

1. Open Settings or another non-Infrastructure page.
2. Activate the header search control.

Actual result: the app navigates to Infrastructure with the page-level device search empty and focus still on the header link. The control does not search services or networks.

Expected result: provide a global search surface for devices, services, and networks, or narrow the label to match the existing device-only behavior; focus the relevant input when entering the page.
