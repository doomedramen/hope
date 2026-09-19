//! Milestone 1 canonical inventory: repository/route layer over the
//! `domain::inventory` policy core (identity scoring, merge/undo model).
//! Spec §17 M1; design docs/design/m1-inventory.md.

pub mod addresses;
pub mod changes;
pub mod devices;
pub mod discovery_scopes;
pub mod events;
pub mod evidence;
pub mod generic;
pub mod identity_service;
pub mod pagination;
pub mod retention;
