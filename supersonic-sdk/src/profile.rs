//! A destination's prior on-chain footprint — the variable the destination layer
//! matches on.
//!
//! This is the product's model of what an observer reads about a bundle leg's
//! **destination** with one `getSignaturesForAddress` call, before the bundle's slot.
//! The adversary harness (`supersonic-dest-harness`) reasons over the same type when it
//! measures the channel, so the thing we defend and the thing we measure are literally
//! one struct.

use serde::{Deserialize, Serialize};

/// A destination's prior on-chain footprint, as of the bundle's slot.
///
/// Every field is obtainable with one `getSignaturesForAddress` call per leg — this is
/// a cheap attack to mount, which is exactly why the destination layer exists.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DestProfile {
    /// Did the address have **any** signature strictly before the bundle slot?
    /// A freshly-derived decoy has `false` by construction — the account does not exist
    /// until the bundle creates it.
    pub exists: bool,
    /// Count of signatures strictly before the bundle slot.
    pub prior_sigs: u32,
    /// Slots between the address's first observed activity and the bundle.
    /// `None` when the address has no prior activity.
    pub age_slots: Option<u64>,
    /// Slots between the address's last activity before the bundle and the bundle.
    /// `None` when the address has no prior activity.
    pub recency_slots: Option<u64>,
}

impl DestProfile {
    /// The profile of a freshly-derived decoy destination: no history of any kind.
    /// This is what a naive `derive_decoy_keypair` produces — and the +0.6-advantage
    /// leak the destination layer closes.
    pub fn fresh() -> Self {
        Self::default()
    }

    /// Build from an observed mainnet destination.
    pub fn observed(prior_sigs: u32, age_slots: Option<u64>, recency_slots: Option<u64>) -> Self {
        Self {
            exists: prior_sigs > 0,
            prior_sigs,
            age_slots,
            recency_slots,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_decoy_has_no_history() {
        let d = DestProfile::fresh();
        assert!(!d.exists);
        assert_eq!(d.prior_sigs, 0);
        assert_eq!(d.age_slots, None);
        assert_eq!(d.recency_slots, None);
    }

    #[test]
    fn observed_sets_exists_from_prior_sigs() {
        assert!(!DestProfile::observed(0, None, None).exists);
        assert!(DestProfile::observed(1, Some(10), Some(2)).exists);
    }
}
