---
id: "006"
title: "Preserve maintenance resource keys"
status: open
priority: medium
created: "2026-09-20T17:11:27Z"
updated: "2026-09-20T17:11:27Z"
tags: ["maintenance", "resources", "data-loss"]
---

## Problem

A maintenance event created with a resource key does not preserve or display that key after saving.

## Context

Steps observed:

1. Create a maintenance event.
2. Add a resource with Identifier `Resource key` and key `network-core`.
3. Save the event.
4. Open the event in the timeline or list, then choose `Edit`.

Actual result: the event displays `Unidentified resource`. The edit form changes the resource to Identifier `Entity`, shows a blank Kind, and shows a generated UUID instead of `network-core`.

Expected result: retain the resource-key mode and value, and display `network-core` in the event details and schedule views.
