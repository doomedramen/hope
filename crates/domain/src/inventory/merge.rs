//! Non-destructive device merge/undo model.
//!
//! Spec §4.2; design docs/design/m1-inventory.md §4. This is the pure,
//! DB-free version of the invariant the server enforces transactionally:
//! merging never re-points or deletes evidence/interface rows, only flips
//! `canonical_of`/`status` metadata, so undo is lossless. Kept here so the
//! invariant is property-tested without a database (see `proptest` module
//! below); `apps/server`'s repository layer mirrors this exactly against
//! real `devices`/`interfaces`/`evidence` tables.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceStatus {
    Active,
    Merged { canonical_of: Uuid },
}

#[derive(Debug, Clone)]
pub struct Device {
    pub id: Uuid,
    pub status: DeviceStatus,
}

/// A minimal in-memory mirror of the observation rows that must survive a
/// merge/undo cycle untouched: interfaces and evidence, keyed by the
/// owning device id exactly as `interfaces.device_id` / `evidence.subject_id`
/// are in the real schema.
#[derive(Debug, Default, Clone)]
pub struct InventoryStore {
    pub devices: HashMap<Uuid, Device>,
    /// device_id -> set of interface ids owned by that device row.
    pub interfaces_by_device: HashMap<Uuid, HashSet<Uuid>>,
    /// device_id -> set of evidence ids whose subject is that device row.
    pub evidence_by_device: HashMap<Uuid, HashSet<Uuid>>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum MergeError {
    #[error("device not found")]
    NotFound,
    #[error("device already merged")]
    AlreadyMerged,
    #[error("cannot merge a device into itself")]
    SelfMerge,
}

impl InventoryStore {
    pub fn add_device(&mut self, id: Uuid) {
        self.devices.entry(id).or_insert(Device {
            id,
            status: DeviceStatus::Active,
        });
        self.interfaces_by_device.entry(id).or_default();
        self.evidence_by_device.entry(id).or_default();
    }

    pub fn add_interface(&mut self, device_id: Uuid, interface_id: Uuid) {
        self.interfaces_by_device
            .entry(device_id)
            .or_default()
            .insert(interface_id);
    }

    pub fn add_evidence(&mut self, device_id: Uuid, evidence_id: Uuid) {
        self.evidence_by_device
            .entry(device_id)
            .or_default()
            .insert(evidence_id);
    }

    /// Resolve a possibly-merged device id to its survivor id, per §4's
    /// `resolve_device` helper.
    pub fn resolve(&self, id: Uuid) -> Uuid {
        match self.devices.get(&id).map(|d| &d.status) {
            Some(DeviceStatus::Merged { canonical_of }) => self.resolve(*canonical_of),
            _ => id,
        }
    }

    /// Aggregate view: every interface/evidence id reachable from `id`
    /// after following merge redirects across all devices whose
    /// `canonical_of` chain resolves to the same survivor.
    pub fn aggregate_interfaces(&self, id: Uuid) -> HashSet<Uuid> {
        let survivor = self.resolve(id);
        self.devices
            .keys()
            .filter(|d| self.resolve(**d) == survivor)
            .flat_map(|d| {
                self.interfaces_by_device
                    .get(d)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect()
    }

    pub fn aggregate_evidence(&self, id: Uuid) -> HashSet<Uuid> {
        let survivor = self.resolve(id);
        self.devices
            .keys()
            .filter(|d| self.resolve(**d) == survivor)
            .flat_map(|d| self.evidence_by_device.get(d).cloned().unwrap_or_default())
            .collect()
    }

    /// Merge `absorbed` into `survivor`. Non-destructive: no interface or
    /// evidence row's owning device id changes, only `absorbed`'s status
    /// metadata.
    pub fn merge(&mut self, survivor: Uuid, absorbed: Uuid) -> Result<(), MergeError> {
        if survivor == absorbed {
            return Err(MergeError::SelfMerge);
        }
        if !self.devices.contains_key(&survivor) || !self.devices.contains_key(&absorbed) {
            return Err(MergeError::NotFound);
        }
        if matches!(self.devices[&absorbed].status, DeviceStatus::Merged { .. }) {
            return Err(MergeError::AlreadyMerged);
        }
        self.devices.get_mut(&absorbed).unwrap().status = DeviceStatus::Merged {
            canonical_of: survivor,
        };
        Ok(())
    }

    /// Undo: flip `absorbed` back to active. O(1) metadata change; no
    /// interface/evidence row was ever moved, so nothing to restore.
    pub fn undo_merge(&mut self, absorbed: Uuid) -> Result<(), MergeError> {
        match self.devices.get_mut(&absorbed) {
            Some(d) if matches!(d.status, DeviceStatus::Merged { .. }) => {
                d.status = DeviceStatus::Active;
                Ok(())
            }
            Some(_) => Err(MergeError::NotFound),
            None => Err(MergeError::NotFound),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_then_undo_restores_independent_ownership() {
        let mut store = InventoryStore::default();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let ia = Uuid::new_v4();
        let ib = Uuid::new_v4();
        store.add_device(a);
        store.add_device(b);
        store.add_interface(a, ia);
        store.add_interface(b, ib);

        store.merge(a, b).unwrap();
        assert_eq!(store.aggregate_interfaces(a), HashSet::from([ia, ib]));

        store.undo_merge(b).unwrap();
        assert_eq!(store.aggregate_interfaces(a), HashSet::from([ia]));
        assert_eq!(store.aggregate_interfaces(b), HashSet::from([ib]));
    }

    #[test]
    fn cannot_merge_into_self() {
        let mut store = InventoryStore::default();
        let a = Uuid::new_v4();
        store.add_device(a);
        assert_eq!(store.merge(a, a), Err(MergeError::SelfMerge));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Property: for any sequence of (device, interface/evidence) creations
    /// followed by a merge and then an undo, the per-device ownership sets
    /// are byte-identical (same ids, same counts) to before the merge.
    /// This is the direct proptest form of the M1 gate "a merge can be
    /// undone without losing observations."
    fn arb_small_uuid_pool(n: usize) -> impl Strategy<Value = Vec<Uuid>> {
        proptest::collection::vec(any::<u128>(), n)
            .prop_map(|nums| nums.into_iter().map(Uuid::from_u128).collect())
    }

    proptest! {
        #[test]
        fn merge_undo_is_lossless(
            device_seed in any::<u128>(),
            other_seed in any::<u128>(),
            interfaces in arb_small_uuid_pool(5),
            evidence in arb_small_uuid_pool(5),
            other_interfaces in arb_small_uuid_pool(5),
            other_evidence in arb_small_uuid_pool(5),
        ) {
            let survivor = Uuid::from_u128(device_seed);
            let absorbed = Uuid::from_u128(other_seed.wrapping_add(1).max(1) ^ 0xDEAD_BEEF);
            prop_assume!(survivor != absorbed);

            let mut store = InventoryStore::default();
            store.add_device(survivor);
            store.add_device(absorbed);
            for i in &interfaces { store.add_interface(survivor, *i); }
            for e in &evidence { store.add_evidence(survivor, *e); }
            for i in &other_interfaces { store.add_interface(absorbed, *i); }
            for e in &other_evidence { store.add_evidence(absorbed, *e); }

            let before_survivor_ifaces = store.interfaces_by_device[&survivor].clone();
            let before_absorbed_ifaces = store.interfaces_by_device[&absorbed].clone();
            let before_survivor_evidence = store.evidence_by_device[&survivor].clone();
            let before_absorbed_evidence = store.evidence_by_device[&absorbed].clone();

            store.merge(survivor, absorbed).unwrap();

            // Aggregate view during the merge must union both sides,
            // nothing dropped.
            let expected_ifaces: HashSet<_> =
                before_survivor_ifaces.union(&before_absorbed_ifaces).cloned().collect();
            prop_assert_eq!(store.aggregate_interfaces(survivor), expected_ifaces);

            store.undo_merge(absorbed).unwrap();

            // After undo, per-device ownership must be pixel-identical to
            // before the merge -- no row was ever re-pointed.
            prop_assert_eq!(&store.interfaces_by_device[&survivor], &before_survivor_ifaces);
            prop_assert_eq!(&store.interfaces_by_device[&absorbed], &before_absorbed_ifaces);
            prop_assert_eq!(&store.evidence_by_device[&survivor], &before_survivor_evidence);
            prop_assert_eq!(&store.evidence_by_device[&absorbed], &before_absorbed_evidence);
        }

        /// Property: resolve() always terminates at an Active device and
        /// aggregate views are symmetric regardless of which merged id you
        /// query from.
        #[test]
        fn resolve_is_consistent_across_merged_ids(
            device_seed in any::<u128>(),
            other_seed in any::<u128>(),
        ) {
            let survivor = Uuid::from_u128(device_seed);
            let absorbed = Uuid::from_u128(other_seed ^ 0xC0FFEE);
            prop_assume!(survivor != absorbed);

            let mut store = InventoryStore::default();
            store.add_device(survivor);
            store.add_device(absorbed);
            store.merge(survivor, absorbed).unwrap();

            prop_assert_eq!(store.resolve(absorbed), survivor);
            prop_assert_eq!(store.resolve(survivor), survivor);
            prop_assert_eq!(store.aggregate_interfaces(absorbed), store.aggregate_interfaces(survivor));
        }
    }
}
