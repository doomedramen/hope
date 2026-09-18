# 0012. `jiff` for time maths, `rrule` for recurrence

## Status
Accepted

## Date
2026-09-18

## Context
The maintenance subsystem (spec §10) needs correct recurrence handling (RFC 5545 rules) and timezone-aware scheduling, including forward-looking validation (§10.4) and overrun detection (§10.5) — areas where date/time arithmetic bugs are especially easy to introduce, particularly around DST transitions. Milestone 9 (§17) explicitly covers the maintenance calendar and conflict engine.

## Decision
Use `jiff` as the time library for all date/time arithmetic throughout the codebase, and `rrule` for RFC 5545 recurrence expansion. Because `rrule` is built on `chrono` internally, convert between `jiff` and `chrono` types only at a thin wrapper boundary around the `rrule` crate, keeping `chrono` out of the rest of the codebase.

## Alternatives considered
- **`chrono` as the primary time library** — rejected. `jiff` has a more correctness-oriented API (explicit handling of ambiguous/DST-affected times, no silent panics on invalid arithmetic), which matters for maintenance-window and check-interval correctness; `chrono` is used only because `rrule` depends on it, not by choice.
- **Hand-rolled RRULE parsing** — rejected: RFC 5545 recurrence rules (including exceptions, `UNTIL`, `COUNT`, `BYDAY` etc.) are intricate enough that reimplementing them risks the exact class of subtle scheduling bugs the maintenance engine most needs to avoid.

## Consequences
A single conversion boundary (the `rrule` wrapper) is the only place `chrono` types appear; everything else uses `jiff`. DST-transition test cases are required for the recurrence wrapper (spec Milestone 9, §17) to catch conversion errors at the boundary, since that is the one place a bug could silently reintroduce chrono's semantics into jiff-based scheduling logic.
