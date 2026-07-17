//! Adversary classifiers over the **destination** channel.
//!
//! Deliberately shaped like `harness/src/classifiers.rs` in PR #1 — same enum
//! surface, same `predict` contract, same lowest-index tie-break — so the two suites
//! compose into one adversary rather than competing. Where that suite reads
//! `amounts: &[u64]`, this one reads `profiles: &[DestProfile]`. They are the two
//! halves of what an observer of a confirmed bundle actually sees.
//!
//! Each classifier is one concrete attack, runnable against a real bundle with one
//! `getSignaturesForAddress` per leg.

use crate::profile::DestProfile;

/// A predicted real-leg index.
pub type Prediction = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestClassifier {
    /// **The attack.** "The real leg is the one whose destination already existed."
    ///
    /// A `derive_decoy_keypair` decoy has no history — the bundle itself creates the
    /// account. A real payee usually does. Measured on mainnet, real SOL transfer
    /// destinations have prior history at a high rate, so this single bit separates
    /// the real leg from every decoy in one step.
    Exists,
    /// "The real leg's destination has the most prior activity." Survives a defense
    /// that gives decoys *some* history but not a matched amount of it.
    MostTxs,
    /// "The real leg's destination is the oldest." The direct analogue of Monero's
    /// `guess-newest` heuristic, mirrored: warmed decoys are younger than an
    /// established payee.
    OldestAge,
    /// "The real leg's destination was active most recently." A warmed pool that goes
    /// quiet between bundles leaks against a payee in continuous use.
    MostRecentlyActive,
}

impl DestClassifier {
    pub fn all() -> &'static [DestClassifier] {
        &[
            DestClassifier::Exists,
            DestClassifier::MostTxs,
            DestClassifier::OldestAge,
            DestClassifier::MostRecentlyActive,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            DestClassifier::Exists => "dest_exists",
            DestClassifier::MostTxs => "dest_most_txs",
            DestClassifier::OldestAge => "dest_oldest_age",
            DestClassifier::MostRecentlyActive => "dest_most_recent",
        }
    }

    /// Predict the real-leg index from the observable destination profiles.
    ///
    /// Ties resolve to the lowest index. As in PR #1's suite, this does not bias the
    /// result: the real leg sits at a uniformly-random position, so when `m` legs tie
    /// the real is equally likely to be any of them and the classifier scores `1/m` —
    /// which is exactly the "no signal" outcome we want a closed channel to produce.
    pub fn predict(&self, profiles: &[DestProfile]) -> Prediction {
        debug_assert!(!profiles.is_empty());
        match self {
            DestClassifier::Exists => argmax_by(profiles, |p| p.exists as u8 as f64),
            DestClassifier::MostTxs => argmax_by(profiles, |p| p.prior_sigs as f64),
            // Oldest = largest age. A profile with no history has no age; treat it as
            // age 0 so it never wins, which is the honest reading (it is not old).
            DestClassifier::OldestAge => argmax_by(profiles, |p| p.age_slots.unwrap_or(0) as f64),
            // Most recent = smallest slots-since-last-activity, so negate. No history
            // means never active: rank it last via +inf distance.
            DestClassifier::MostRecentlyActive => argmax_by(profiles, |p| {
                -(p.recency_slots.map(|r| r as f64).unwrap_or(f64::INFINITY))
            }),
        }
    }
}

/// Index of the maximum of `key(item)`, lowest index on ties.
fn argmax_by<T, F: Fn(&T) -> f64>(xs: &[T], key: F) -> usize {
    let mut best = 0usize;
    let mut best_v = f64::NEG_INFINITY;
    for (i, x) in xs.iter().enumerate() {
        let v = key(x);
        if v > best_v {
            best_v = v;
            best = i;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> DestProfile {
        DestProfile::fresh()
    }

    #[test]
    fn exists_finds_the_only_established_destination() {
        // PR #1's construction verbatim: K-1 fresh decoys, one real payee with
        // history. The real leg is at index 2.
        let profiles = vec![
            fresh(),
            fresh(),
            DestProfile::observed(974, Some(9_000_000), Some(120)),
            fresh(),
        ];
        assert_eq!(DestClassifier::Exists.predict(&profiles), 2);
    }

    #[test]
    fn exists_has_no_signal_when_every_leg_is_established() {
        // What a *defended* bundle looks like: the pool is matched, every leg has
        // history, so the bit carries nothing and the classifier degenerates to a
        // fixed guess — scoring 1/K over uniformly-placed real legs.
        let profiles = vec![
            DestProfile::observed(800, Some(8_000_000), Some(300)),
            DestProfile::observed(974, Some(9_000_000), Some(120)),
            DestProfile::observed(1200, Some(7_500_000), Some(90)),
        ];
        assert_eq!(DestClassifier::Exists.predict(&profiles), 0);
    }

    #[test]
    fn exists_has_no_signal_when_every_leg_is_fresh() {
        // PR #1's *harness* condition — the channel held constant. This is the case
        // its amount-channel measurement is valid under, and the case deployment
        // never produces.
        let profiles = vec![fresh(), fresh(), fresh(), fresh()];
        assert_eq!(DestClassifier::Exists.predict(&profiles), 0);
    }

    #[test]
    fn most_txs_beats_a_thinly_warmed_pool() {
        // A defense that warms decoys to ~10 txs still loses to a payee with ~1000.
        let profiles = vec![
            DestProfile::observed(11, Some(400_000), Some(5_000)),
            DestProfile::observed(9, Some(380_000), Some(6_100)),
            DestProfile::observed(974, Some(9_000_000), Some(120)),
        ];
        assert_eq!(DestClassifier::MostTxs.predict(&profiles), 2);
    }

    #[test]
    fn oldest_age_ranks_no_history_as_not_old() {
        let profiles = vec![fresh(), DestProfile::observed(3, Some(500), Some(10))];
        assert_eq!(DestClassifier::OldestAge.predict(&profiles), 1);
    }

    #[test]
    fn most_recent_ranks_no_history_last() {
        let profiles = vec![
            fresh(),
            DestProfile::observed(3, Some(500), Some(10)),
            DestProfile::observed(3, Some(500), Some(4)),
        ];
        assert_eq!(DestClassifier::MostRecentlyActive.predict(&profiles), 2);
    }
}
