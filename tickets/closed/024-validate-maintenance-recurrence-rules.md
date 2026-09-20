---
id: "024"
title: "Validate maintenance recurrence rules before submit"
status: closed
priority: medium
created: "2026-09-20T17:53:26Z"
updated: "2026-09-20T19:53:40Z"
tags: ["maintenance", "recurrence", "validation", "usability"]
---

## Problem

The recurrence field accepts malformed RRULE text and only reports a technical parser error after a submit request. The form should validate the rule locally and explain the supported format before contacting the server.

## Context

After adding a resource, submitting `not-an-rrule` for `QA invalid recurrence` produced `encountered unexpected or invalid data: invalid recurrence_rule: RRule parsing error: ... malformed property parameter`. Surface a concise inline validation message and an example such as `FREQ=WEEKLY;BYDAY=SA`.
