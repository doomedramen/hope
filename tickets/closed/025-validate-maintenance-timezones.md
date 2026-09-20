---
id: "025"
title: "Validate maintenance timezones in the form"
status: closed
priority: medium
created: "2026-09-20T17:53:26Z"
updated: "2026-09-20T19:53:40Z"
tags: ["maintenance", "timezone", "validation", "usability"]
---

## Problem

The maintenance form accepts an arbitrary timezone string and submits it to the server, which returns a technical error. Operators need a supported timezone selector or inline validation before saving an event.

## Context

Submitting `Not/AZone` for `QA invalid timezone` left the dialog open with `unsupported timezone \`Not/AZone\`: failed to find time zone \`Not/AZone\` in time zone database`. Replace the free-form field with a validated IANA timezone choice or provide a clear field-level error.
