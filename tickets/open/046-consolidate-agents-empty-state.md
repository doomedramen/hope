---
id: "046"
title: "Consolidate the empty Agents experience into one onboarding state"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["agents", "empty-state", "onboarding", "ux"]
---

## Problem

When there are no agents, the page shows a searchable "Enrolled agents" panel
with "No enrolled agents" and an Enroll button, alongside a second panel that
says "Select an agent." The latter has no valid action in this state.

The empty state also retains a search field and four zero-value status cards,
which dilute the enrollment path rather than explaining the first useful step.

## Context

This was observed with zero agents on both desktop and a 375px phone viewport.
The page already has an explicit "Enroll agent" action, but it appears in
three separate places with unrelated empty-state content.
