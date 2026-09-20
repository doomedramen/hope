---
id: "050"
title: "Keep maintenance form actions discoverable on small screens"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["maintenance", "mobile", "forms", "dialog", "ux"]
---

## Problem

At 375x812, the Create maintenance event dialog opens at the top of a long
scrolling form. Its Cancel and Create event actions are outside the initial
visible region, and the first screen shows only the beginning of the next
section with no persistent action area.

An operator must discover and traverse the internal dialog scroll before they
can find the form's completion or dismissal actions.

## Context

The dialog correctly constrains its height and supports internal scrolling.
This finding concerns the initial mobile presentation of the long form, not
horizontal overflow or dialog width.
