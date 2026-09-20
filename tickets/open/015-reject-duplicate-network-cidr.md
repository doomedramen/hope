---
id: "015"
title: "Reject duplicate network CIDRs"
status: open
priority: medium
created: "2026-09-20T17:36:00Z"
updated: "2026-09-20T17:36:00Z"
tags: ["infrastructure", "networks", "validation", "data-integrity"]
---

## Problem

The Add network form accepts a second network with the same CIDR as an existing network.

## Context

Submitting `QA duplicate network` with CIDR `192.168.1.0/24` succeeded while `home 192.168.1.0/24` already existed. Infrastructure then displayed two configured network boundaries with identical CIDRs, which makes discovery scope selection ambiguous. The form should reject duplicate or overlapping boundaries with a clear validation message.
