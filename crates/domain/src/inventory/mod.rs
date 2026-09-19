//! Milestone 1 canonical inventory domain types.
//!
//! Spec §17 M1; design docs/design/m1-inventory.md. This module holds the
//! DB-free, unit/property-testable core: identity scoring
//! ([`identity`]) and the non-destructive merge/undo model ([`merge`]).
//!
//! NOTE: the `sqlx` repository layer (entity structs bound to
//! `devices`/`interfaces`/`evidence`/... rows, and the `/api/v1` handlers
//! and worker retention job that consume it) is **not yet implemented** —
//! see the M1 session notes for the stub/deviation list. This module is
//! the policy core those layers are meant to call into.

pub mod identity;
pub mod merge;
