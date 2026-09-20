---
id: "014"
title: "Humanize selected merge target"
status: closed
priority: medium
created: "2026-09-20T17:28:31Z"
updated: "2026-09-20T19:53:40Z"
tags: ["infrastructure", "devices", "merge", "usability"]
---

## Problem

The Merge device dialog displays the selected survivor as a full UUID in the collapsed `Into survivor` field, even though the expanded options provide human-readable device names and short IDs.

## Context

On Infrastructure, selecting `catacomb` and opening `Merge` showed the collapsed target value `d437e927-48e5-4b6e-82d1-5e3c39810c3e`. Expanding the same control showed the more usable label `Unnamed device · d437e927`. Keep the selected value consistent with the option label so operators can verify the merge target without decoding a UUID.
