---
id: "037"
title: "Fix light-theme semantic text contrast"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["accessibility", "color", "design-system"]
---

## Problem

Several light-theme semantic foreground/background pairs fall below the 4.5:1
contrast threshold for normal text. Calculations from the declared OKLCH
tokens found approximately 4.43:1 for primary button text, 4.45:1 for
attention-status text, and 4.38:1 for critical-status text.

These colors are used by normal-size button and badge labels, including primary
actions and critical or attention states that operators need to read quickly.

## Context

The affected values are declared in `apps/web/src/index.css` and consumed by
the shared Button and Badge components. Dark-theme values were not part of this
finding.
