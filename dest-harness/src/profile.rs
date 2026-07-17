
//! What an observer can learn about a bundle leg's **destination**, using only
//! public RPC, before the bundle's own slot.
//!
//! This is the channel `harness/src/classifiers.rs` (PR #1) deliberately holds
//! constant:
//!
//! > "in the harness every destination (real and decoy) is a fresh key, so the
//! > destination channel is held constant to isolate the amount/position channel"
//!
//! Holding it constant is correct method for measuring the amount channel. It is not
//! a property of deployment: on mainnet a real payee has history and a derived decoy
//! has none, so the channel carries most of the signal. This module models what the
//! adversary reads.

use serde::{Deserialize, Serialize};

/// A destination's prior on-chain footprint, as of the bundle's slot.
///
/// Every field is obtainable with one `getSignaturesForAddress` call per leg —
/// this is a cheap attack, not a research-grade one.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DestProfile {
    /// Did the address have **any** signature strictly before the bundle slot?
    /// For a `derive_decoy_keypair` decoy this is always false; the address does
    /// not exist until the bundle creates it.
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
    /// This is what PR #1's `derive_decoy_keypair` produces, by construction.
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

/// One sampled bundle: `K` destination profiles in transaction order, plus the index
/// of the real leg (which the adversary is trying to recover, and which the harness
/// knows only for scoring).
#[derive(Clone, Debug)]
pub struct Bundle {
    pub profiles: Vec<DestProfile>,
    pub real_index: usize,
}

impl Bundle {
    pub fn k(&self) -> usize {
        self.profiles.len()
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
