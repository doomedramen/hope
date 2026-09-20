---
id: "039"
title: "Increase shared control touch targets on mobile"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["mobile", "touch", "accessibility", "design-system"]
---

## Problem

The shared mobile control sizes are largely desktop-density dimensions. Default
buttons and inputs are 32px tall, small buttons are 28px tall, icon buttons
are 24px to 32px, and mobile primary-navigation links use a 40px minimum
height.

At a 375px phone width, common actions such as Refresh, filter chips, form
fields, and icon controls are visually and physically small for touch use.

## Context

The dimensions come from the shared Button, Input, Select, and navigation
styles. The review used 375x812 and showed these controls throughout Agents,
Settings, Networks, Monitoring, and Maintenance.
