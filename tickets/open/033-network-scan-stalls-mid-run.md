---
id: "033"
title: "Investigate network scans stalling mid-run"
status: open
priority: high
created: "2026-09-20T21:26:09Z"
updated: "2026-09-20T21:26:09Z"
tags: ["infrastructure", "networks", "discovery", "scanning", "performance"]
---

## Problem

Network scans stall at “3 of 254 confirmed targets completed. 197,117 of 16,645,890 ports completed.”

## Context

The scan should continue making progress after this point; investigate the worker and scan execution path to determine why a full network discovery can stop advancing.
