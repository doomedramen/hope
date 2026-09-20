---
id: "002"
title: "Validate network CIDR before submission"
status: closed
priority: medium
created: "2026-09-20T17:11:27Z"
updated: "2026-09-20T19:53:40Z"
tags: ["infrastructure", "validation", "networks"]
---

## Problem

The Add network form enables submission for an invalid CIDR and then exposes a raw database error instead of showing field-level validation.

## Context

Steps observed:

1. Open Infrastructure and choose `Add network`.
2. Enter `QA invalid network` as Name.
3. Enter `not-a-cidr` as CIDR.
4. Submit the form.

Actual result: the form accepts the invalid value and shows `Action failed` with `error returned from database: invalid input syntax for type cidr: "not-a-cidr"`.

Expected result: reject malformed CIDR input before the request, identify the CIDR field, and avoid exposing database implementation errors.
