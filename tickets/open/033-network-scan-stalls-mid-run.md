---
id: "033"
title: "Investigate network scans stalling mid-run"
status: open
priority: high
created: "2026-09-20T21:26:09Z"
updated: "2026-09-20T21:57:00Z"
tags: ["infrastructure", "networks", "discovery", "scanning", "performance"]
---

## Problem

Network scans stall at “3 of 254 confirmed targets completed. 197,117 of 16,645,890 ports completed.”

## Context

The scan should continue making progress after this point; investigate the worker and scan execution path to determine why a full network discovery can stop advancing.

## Diagnosis

The worker enumerates a `/24` scope in ascending address order, so the reported
progress is consistent with completing `.1` through `.3` and then reaching
`192.168.1.4`. An inactive address is not skipped: each TCP connect timeout or
unclassified error becomes a `filtered` observation, and the worker persists
those observations as progress.

The apparent stall is the cost of probing an inactive host. Targets are scanned
serially, the default per-host concurrency is 2, the connect timeout is 1
second, and progress is written after each 256-port batch. A fully filtered
batch can therefore take about 128 seconds; a full 65,535-port inactive host
can take roughly 9 hours in the worst case.

The existing timeout and filtered-observation tests pass, but there is no real
inactive-host timing regression test. The ticket remains open for the worker
and scan-mode changes needed to make this behavior usable.

## Proposal (not yet approved)

Use a curated common-port scan for initial discovery, keep full TCP explicit,
and add host/batch progress plus a bounded inactive-host or liveness path for
full scans.
