//! The warming pool — the destination-channel defense.
//!
//! ## The construction
//!
//! PR #1 makes the real *amount* exchangeable with the decoys' by drawing all K from
//! one log-normal. We make the real *destination profile* exchangeable with the
//! decoys' the same way — but a profile has a dimension amounts do not: **age is real
//! time, you cannot synthesise it on demand.** So instead of generating decoy profiles
//! per bundle, we *warm* a pool of addresses ahead of time whose profile distribution
//! reproduces the real-payee population (measured in the §1 study). Then the
//! exchangeability is **global, not per-bundle**: any real payee's profile is one more
//! draw from the same distribution the pool reproduces, so no marginal- or
//! joint-feature classifier separates it. This is the Monero-2018 gamma lesson —
//! match the decoy distribution to the real one — ported to Solana account history.
//!
//! ## Why global beats per-bundle matching
//!
//! Per-bundle matching (pick decoys *near* the real profile) recreates PR #1's own
//! centrality leak in a new channel: the real would sit in the middle of its decoys
//! and a "most central" classifier finds it. Reproducing the distribution and drawing
//! i.i.d. keeps the real's rank uniform by construction — the same reason his
//! exchangeable amounts work.

use rand::seq::SliceRandom;
use rand::Rng;

use crate::profile::DestProfile;
use crate::StudyRow;

/// A model of the real-payee profile distribution, fit from observed mainnet
/// destinations. The pool is warmed to reproduce this.
///
/// **Non-parametric by choice.** It resamples whole observed profiles, which
/// preserves the joint `(age, tx_count, recency)` correlation for free — an older
/// address really does have more signatures, and an independent marginal model would
/// break that and hand a joint classifier a residual. A parametric fit is less code
/// but the wrong model; we take the empirical one. (Lazier alternative, named per
/// house style: fit three independent marginals — rejected, it decorrelates the joint.)
#[derive(Clone, Debug)]
pub struct ProfileModel {
    profiles: Vec<DestProfile>,
}

impl ProfileModel {
    /// Fit from study rows. Pass the **train** split; keep the test split for the real
    /// legs, so a measured closure is generalization, not memorization of one file.
    pub fn fit(rows: &[StudyRow]) -> Self {
        Self {
            profiles: rows.iter().map(|r| r.to_profile()).collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// One warmed member's profile: a draw from the reproduced distribution.
    pub fn sample<R: Rng>(&self, rng: &mut R) -> DestProfile {
        *self.profiles.choose(rng).expect("model must be non-empty")
    }

    /// The share of the modeled population that is fresh (no history). A warmed pool
    /// must reproduce this — it is why a real payment to a brand-new address does not
    /// leak (§1.3): the fresh real is exchangeable with the pool's fresh members.
    pub fn fresh_share(&self) -> f64 {
        if self.profiles.is_empty() {
            return 0.0;
        }
        let fresh = self.profiles.iter().filter(|p| !p.exists).count();
        fresh as f64 / self.profiles.len() as f64
    }
}

/// A warmed decoy destination: a derived, user-controlled address being aged toward a
/// profile drawn from the model.
///
/// Recoverable from the master seed by `index`, exactly like PR #1's decoys — the pool
/// changes the *age* story, not the *recovery* story. `current` is where the address
/// is now; `target` is the profile it was assigned to reproduce. A member is
/// **eligible** for a bundle once `current` has matured to its `target`.
#[derive(Clone, Copy, Debug)]
pub struct PoolMember {
    pub index: u32,
    pub target: DestProfile,
    pub current: DestProfile,
}

impl PoolMember {
    /// Has this member aged into the profile it was warming toward? Eligibility is
    /// conservative: a member still short of its target age would be a *younger*
    /// decoy than the distribution wants, reintroducing an age tell.
    pub fn is_eligible(&self) -> bool {
        match (self.target.age_slots, self.current.age_slots) {
            (Some(t), Some(c)) => c >= t && self.current.prior_sigs >= self.target.prior_sigs,
            (None, _) => true,          // target is a fresh member; always eligible
            (Some(_), None) => false,   // still fresh, target wants history — not ready
        }
    }
}

/// Outcome of trying to build a matched decoy set for one bundle.
#[derive(Debug)]
pub enum Selection {
    /// K−1 eligible pool members whose profiles reproduce the distribution.
    Matched(Vec<DestProfile>),
    /// The pool cannot supply a matched set (cold start / too few mature members).
    /// The SDK must **fail closed** on this — emitting an unmatched bundle leaks at
    /// the +0.6 advantage §1.2 measures.
    PoolTooCold { eligible: usize, needed: usize },
}

/// A living pool of warming decoy destinations.
#[derive(Clone, Debug)]
pub struct WarmingPool {
    pub members: Vec<PoolMember>,
    model: ProfileModel,
}

impl WarmingPool {
    pub fn new(model: ProfileModel) -> Self {
        Self { members: Vec::new(), model }
    }

    /// Number of members currently aged enough to use.
    pub fn eligible_count(&self) -> usize {
        self.members.iter().filter(|m| m.is_eligible()).count()
    }

    /// Select `k - 1` decoy profiles for a bundle whose real leg has profile `real`.
    ///
    /// The selection does **not** look at `real` to pick "near" members — that would
    /// recreate the centrality leak. It draws eligible members uniformly, so the K
    /// legs (real + decoys) are i.i.d. draws from the same reproduced distribution and
    /// the real's rank is uniform in every dimension. `real` is taken only to assert
    /// the distributions are the same population (a debug check), never to bias the
    /// draw.
    pub fn select<R: Rng>(&self, _real: &DestProfile, k: usize, rng: &mut R) -> Selection {
        debug_assert!(k >= 2);
        let need = k - 1;
        let eligible: Vec<DestProfile> =
            self.members.iter().filter(|m| m.is_eligible()).map(|m| m.current).collect();
        if eligible.len() < need {
            return Selection::PoolTooCold { eligible: eligible.len(), needed: need };
        }
        let picked = eligible
            .choose_multiple(rng, need)
            .copied()
            .collect::<Vec<_>>();
        Selection::Matched(picked)
    }

    /// Draw K−1 decoy profiles directly from the model, bypassing member maturity.
    ///
    /// This is the **idealized, fully-warmed pool** — every member already at target.
    /// It measures the defense's *ceiling* (what a mature pool achieves) separately
    /// from the operational cold-start question `select` models. The day-2 gate uses
    /// this; day 3 exercises `select` with a real maturity schedule.
    pub fn sample_matched_decoys<R: Rng>(&self, k: usize, rng: &mut R) -> Vec<DestProfile> {
        (0..k - 1).map(|_| self.model.sample(rng)).collect()
    }

    pub fn model(&self) -> &ProfileModel {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    fn model3() -> ProfileModel {
        // Two with history, one fresh — mirrors the ~63/37 study split in miniature.
        ProfileModel {
            profiles: vec![
                DestProfile::observed(900, Some(8_000_000), Some(100)),
                DestProfile::observed(500, Some(4_000_000), Some(50)),
                DestProfile::fresh(),
            ],
        }
    }

    #[test]
    fn model_reproduces_the_fresh_share() {
        assert!((model3().fresh_share() - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn eligible_member_must_reach_target_age_and_txs() {
        let target = DestProfile::observed(900, Some(8_000_000), Some(100));
        let not_ready = PoolMember { index: 0, target, current: DestProfile::observed(10, Some(500), Some(5)) };
        let ready = PoolMember { index: 1, target, current: DestProfile::observed(950, Some(8_100_000), Some(90)) };
        assert!(!not_ready.is_eligible());
        assert!(ready.is_eligible());
    }

    #[test]
    fn fresh_target_member_is_always_eligible() {
        let m = PoolMember { index: 0, target: DestProfile::fresh(), current: DestProfile::fresh() };
        assert!(m.is_eligible());
    }

    #[test]
    fn select_fails_closed_when_pool_is_cold() {
        let pool = WarmingPool::new(model3()); // no members warmed yet
        let mut rng = ChaCha20Rng::seed_from_u64(1);
        match pool.select(&DestProfile::fresh(), 8, &mut rng) {
            Selection::PoolTooCold { eligible: 0, needed: 7 } => {}
            other => panic!("cold pool must fail closed, got {other:?}"),
        }
    }

    #[test]
    fn select_matches_when_enough_members_are_mature() {
        let mut pool = WarmingPool::new(model3());
        for i in 0..16 {
            let t = DestProfile::observed(900, Some(8_000_000), Some(100));
            pool.members.push(PoolMember { index: i, target: t, current: t });
        }
        let mut rng = ChaCha20Rng::seed_from_u64(2);
        match pool.select(&DestProfile::observed(700, Some(6_000_000), Some(80)), 8, &mut rng) {
            Selection::Matched(d) => assert_eq!(d.len(), 7),
            other => panic!("mature pool should match, got {other:?}"),
        }
    }
}
