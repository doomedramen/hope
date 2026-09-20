---
id: "011"
title: "Simplify scan setup and launch flow"
status: closed
priority: medium
created: "2026-09-20T17:11:27Z"
updated: "2026-09-20T19:53:40Z"
tags: ["infrastructure", "discovery", "scanning", "ux"]
---

## Problem

Launching an initial discovery scan requires a long multi-step flow and exposes a very large port-sweep scope without a compact summary or guided action.

## Context

Observed flow for `192.168.1.0/24`:

1. Choose `Calculate targets`.
2. Check `I reviewed the target count`.
3. Choose `Confirm scope`.
4. Choose `Launch initial discovery`.

The UI reported 254 targets and 16,645,890 ports, then showed a queued status and a separate `Cancel scan` action.

Expected improvement: reduce unnecessary transitions, explain scan cost and duration before launch, and make the safe default action clear without hiding the scope review.
