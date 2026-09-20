---
id: "003"
title: "Prevent unnamed device creation"
status: open
priority: medium
created: "2026-09-20T17:11:27Z"
updated: "2026-09-20T17:11:27Z"
tags: ["infrastructure", "validation", "devices"]
---

## Problem

The Add device form creates a device record when Device name is empty, even though the field is presented as required.

## Context

Steps observed:

1. Open Infrastructure and choose `Add device`.
2. Leave Device name empty.
3. Submit `Create device`.

Actual result: a new `Unnamed device` record is created and selected. The device count increases from 4 to 5.

Expected result: block submission, identify the required Device name field, and do not create an unnamed record.
