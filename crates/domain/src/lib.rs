//! Core domain types shared across the server, workers, and (eventually)
//! generated API clients. Spec §14.1: resources are addressed by stable
//! UUIDs, never IP addresses.
//!
//! This crate only holds identity/value types that have no code yet
//! elsewhere in Milestone 0. Larger domain models (devices, services,
//! networks, ...) arrive in Milestone 1 per spec §17.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod discovery;
pub mod fingerprinting;
pub mod inventory;

/// A user account, as created by the setup/bootstrap flow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserId(pub Uuid);

impl UserId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for UserId {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_id_new_is_unique() {
        assert_ne!(UserId::new(), UserId::new());
    }
}
