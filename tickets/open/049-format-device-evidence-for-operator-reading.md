---
id: "049"
title: "Format device evidence for operator reading"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["devices", "evidence", "content-design", "ux"]
---

## Problem

The selected device's Evidence section displays scan facts as raw, truncated
JSON, for example `{"address":"192.168.1.1","port":28082,"transport":…}`
beside a generic "Open Port" label and score.

This requires operators to parse serialized implementation data to learn the
address, port, transport, source, and confidence of a finding. The same device
page also presents multiple indistinguishable "Unnamed device" records, which
makes that evidence harder to associate with a real asset.

## Context

The desktop device detail view showed several raw Open Port entries, while the
phone view retains the same content below the inventory list. Structured values
are currently converted with the shared `displayValue` helper.
