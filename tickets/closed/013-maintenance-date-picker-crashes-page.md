---
id: "013"
title: "Fix maintenance date picker page crash"
status: closed
priority: high
created: "2026-09-20T17:25:30Z"
updated: "2026-09-20T19:53:40Z"
tags: ["maintenance", "date-picker", "crash"]
---

## Problem

Opening the maintenance event date picker crashes the hosted page.

## Context

Steps observed:

1. Open Maintenance.
2. Choose `New event`.
3. Activate `Show local date and time picker` for the Start field.

Actual result: the browser leaves the app and shows `This page crashed` with `192.168.1.242 crashed unexpectedly`.

Expected result: open a date/time picker or leave the form usable without crashing the page.
