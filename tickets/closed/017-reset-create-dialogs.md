---
id: "017"
title: "Reset create dialogs after use"
status: closed
priority: medium
created: "2026-09-20T17:37:57Z"
updated: "2026-09-20T19:53:40Z"
tags: ["infrastructure", "devices", "networks", "dialogs", "usability"]
---

## Problem

The Add device and Add network dialogs retain their previous form values after a successful submission and after reopening.

## Context

After creating `QA VM device`, reopening Add device showed `QA VM device` and device type `Vm` still populated. After creating `QA duplicate network`, reopening Add network showed the previous name and CIDR still populated. Opening a fresh create form should not carry forward stale values from the last record.
