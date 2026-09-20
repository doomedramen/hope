---
id: "012"
title: "Persist discovery scope and scan state across navigation"
status: closed
priority: medium
created: "2026-09-20T17:11:27Z"
updated: "2026-09-20T19:53:40Z"
tags: ["infrastructure", "discovery", "scanning", "state"]
---

## Problem

Infrastructure loses discovery scope and scan-run state when the user navigates away and returns.

## Context

Steps observed:

1. Calculate targets for `192.168.1.0/24`.
2. Acknowledge the target count and confirm the scope.
3. Launch initial discovery.
4. Navigate to another page, then return to Infrastructure.

Actual result: the network returns to `Not set up`, the scan status and cancellation control are gone, and the UI shows `Calculate targets` again even though the scan request was already accepted.

Expected result: reload the confirmed scope and active scan status from the server so navigation and page reload preserve accurate workflow state and do not invite duplicate launches.
