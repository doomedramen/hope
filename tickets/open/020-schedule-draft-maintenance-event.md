---
id: "020"
title: "Make Schedule transition draft events"
status: open
priority: high
created: "2026-09-20T17:42:48Z"
updated: "2026-09-20T17:42:48Z"
tags: ["maintenance", "lifecycle", "workflow"]
---

## Problem

The Schedule action on a draft maintenance event reports no error but leaves the event in Draft state.

## Context

I created `QA draft event` with Initial state `Draft`, then clicked Schedule. The event version increased from v1 to v2, but the UI and a subsequent page reload still showed Draft and the Schedule button. The action should either transition the event to Scheduled or surface a clear failure.
