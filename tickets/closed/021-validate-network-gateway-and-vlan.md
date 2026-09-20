---
id: "021"
title: "Validate network gateway and VLAN fields"
status: closed
priority: medium
created: "2026-09-20T17:53:26Z"
updated: "2026-09-20T19:53:40Z"
tags: ["infrastructure", "networks", "validation", "usability"]
---

## Problem

The Add network form allows malformed gateway and out-of-range VLAN input without an actionable inline validation message. A malformed gateway is submitted to the API and displayed as a raw database error; an out-of-range VLAN can be typed while the submit control remains enabled, but the browser prevents submission without explaining why.

## Context

With `QA invalid gateway`, CIDR `192.168.2.0/24`, and gateway `not-an-ip`, submitting produced `error returned from database: invalid input syntax for type inet: "not-an-ip"`. Changing the gateway to `192.168.2.1` and entering VLAN `4096` left the form open with no VLAN-specific feedback. Validate gateway syntax and VLAN 1–4094 before submission and show the offending field inline.
