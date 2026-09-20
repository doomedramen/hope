---
id: "016"
title: "Validate discovery exclusions inline"
status: open
priority: medium
created: "2026-09-20T17:36:00Z"
updated: "2026-09-20T17:36:00Z"
tags: ["infrastructure", "discovery", "validation", "usability"]
---

## Problem

The discovery scope form accepts an invalid exclusion value and only reports the failure after submitting it to the server.

## Context

Entering `not-a-cidr` in Excluded addresses or ranges and choosing Calculate targets produced the generic `Scope action failed` message with `invalid CIDR \`not-a-cidr\``. Validate each exclusion inline and identify the offending line before making the scope request.
