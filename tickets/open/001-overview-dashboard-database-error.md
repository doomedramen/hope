---
id: "001"
title: "Fix overview dashboard database error"
status: open
priority: medium
created: "2026-09-20T17:11:27Z"
updated: "2026-09-20T17:11:27Z"
tags: ["overview", "dashboard", "database"]
---

## Problem

The hosted Overview page reports that some dashboard data is unavailable and shows the database error `syntax error at or near "monitors"`.

## Context

Steps observed:

1. Open `http://192.168.1.242:8183/` while authenticated.
2. Observe the Overview dashboard.
3. Open Monitoring and select the Monitors view.

Actual result: the Overview dashboard renders partial data with the error banner, and the Monitoring > Monitors view shows the same database error. The `Retry` button does not remove the error during the observed load.

Expected result: Overview loads its dashboard data without exposing a database syntax error to the user.
