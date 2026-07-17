//! The warming pool — the destination-channel defense.
//!
//! ## The construction
//!
//! The amount layer (§`amounts`) makes the real *amount* exchangeable with the decoys'
//! by drawing all K from one log-normal. This module makes the real *destination
//! profile* exchangeable with the decoys' the same way — but a profile has a dimension
//! amounts do not: **age is real time, you cannot synthesise it on demand.** So instead
//! of generating decoy profiles per bundle, we *warm* a pool of addresses ahead of time
//! whose profile distribution reproduces the real-payee population (measured in the §1
//! study). Then the exchangeability is **global, not per-bundle**: any real payee's
//! profile is one more draw from the same distribution the pool reproduces, so no
//! marginal- or joint-feature classifier separates it. This is the Monero-2018 gamma
//! lesson — match the decoy distribution to the real one — ported to Solana account
//! history.
//!
//! ## Why global beats per-bundle matching
//!
//! Per-bundle matching (pick decoys *near* the real profile) recreates the amount
//! layer's original centrality leak in a new channel: the real would sit in the middle
//! of its decoys and a "most central" classifier finds it. Reproducing the distribution
//! and drawing i.i.d. keeps the real's rank uniform by construction.

use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::profile::DestProfile;

/// A model of the real-payee profile distribution, fit from observed mainnet
/// destinations. The pool is warmed to reproduce this.
///
/// **Non-parametric by choice.** It resamples whole observed profiles, which preserves
/// the joint `(age, tx_count, recency)` correlation for free — an older address really
/// does have more signatures, and an independent marginal model would break that and
/// hand a joint classifier a residual. A parametric fit is less code but the wrong
/// model; we take the empirical one.
#[derive(Clone, Debug)]
pub struct ProfileModel {
    profiles: Vec<DestProfile>,
}

impl ProfileModel {
    /// Fit from observed destination profiles. The measurement harness feeds the
    /// **train** split here (converting its study rows to profiles) and keeps the test
    /// split for the real legs, so a measured closure is generalization, not
    /// memorization of one file.
    pub fn from_profiles(profiles: impl IntoIterator<Item = DestProfile>) -> Self {
        Self {
            profiles: profiles.into_iter().collect(),
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
    /// leak: the fresh real is exchangeable with the pool's fresh members.
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
/// Recoverable from the master seed by `index` (see [`crate::derive_pool_keypair`]) —
/// the pool changes the *age* story, not the *recovery* story. `current` is where the
/// address is now; `target` is the profile it was assigned to reproduce. A member is
/// **eligible** for a bundle once `current` has matured to its `target`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PoolMember {
    pub index: u32,
    pub target: DestProfile,
    pub current: DestProfile,
}

impl PoolMember {
    /// Has this member aged into the profile it was warming toward? Eligibility is
    /// conservative: a member still short of its target age would be a *younger* decoy
    /// than the distribution wants, reintroducing an age tell.
    pub fn is_eligible(&self) -> bool {
        match (self.target.age_slots, self.current.age_slots) {
            (Some(t), Some(c)) => c >= t && self.current.prior_sigs >= self.target.prior_sigs,
            (None, _) => true,        // target is a fresh member; always eligible
            (Some(_), None) => false, // still fresh, target wants history — not ready
        }
    }
}

/// Outcome of trying to build a matched decoy set for one bundle.
#[derive(Debug)]
pub enum Selection {
    /// K−1 eligible pool members whose profiles reproduce the distribution. Carries the
    /// members (not just their profiles) so the caller can derive each one's address by
    /// `index`.
    Matched(Vec<PoolMember>),
    /// Too few mature members to supply `needed` decoys. The SDK **fails closed** on this
    /// — emitting an unmatched bundle leaks at the +0.6 advantage the harness measures.
    PoolTooCold { eligible: usize, needed: usize },
    /// Enough mature members, but they do **not** reproduce the modeled fresh/history
    /// split — e.g. only the fresh members have matured, so a uniform draw yields an
    /// all-fresh decoy set and a real leg *with* history stands out as the only aged one
    /// (the very leak this layer closes). Also fail-closed: counting eligible members is
    /// not enough, they must reproduce the distribution (DESIGN §3.2).
    PoolNotRepresentative {
        eligible: usize,
        eligible_fresh_share: f64,
        model_fresh_share: f64,
    },
}

/// How far the eligible pool's fresh/history split may drift from the model before the
/// pool is considered not-yet-representative. Deterministic in pool state (no RNG), so
/// this gate never flakes.
const FRESH_SHARE_TOL: f64 = 0.15;

/// A living pool of warming decoy destinations.
#[derive(Clone, Debug)]
pub struct WarmingPool {
    pub members: Vec<PoolMember>,
    model: ProfileModel,
}

impl WarmingPool {
    pub fn new(model: ProfileModel) -> Self {
        Self {
            members: Vec::new(),
            model,
        }
    }

    /// Number of members currently aged enough to use.
    pub fn eligible_count(&self) -> usize {
        self.members.iter().filter(|m| m.is_eligible()).count()
    }

    /// Select `k - 1` decoy members for a bundle whose real leg has profile `real`.
    ///
    /// The selection does **not** look at `real` to pick "near" members — that would
    /// recreate the centrality leak. It draws eligible members uniformly, so the K legs
    /// (real + decoys) are i.i.d. draws from the same reproduced distribution and the
    /// real's rank is uniform in every dimension. `real` is taken only for the caller's
    /// debug assertion that the populations match, never to bias the draw.
    pub fn select<R: Rng>(&self, _real: &DestProfile, k: usize, rng: &mut R) -> Selection {
        debug_assert!(k >= 2);
        let need = k - 1;
        let eligible: Vec<PoolMember> =
            self.members.iter().filter(|m| m.is_eligible()).copied().collect();
        if eligible.len() < need {
            return Selection::PoolTooCold {
                eligible: eligible.len(),
                needed: need,
            };
        }
        // Count is not enough: the eligible members must reproduce the model's fresh/
        // history split, or a uniform draw is non-exchangeable with a real leg. A pool
        // where only the fresh members have matured passes the count check but leaks.
        let model_fresh = self.model.fresh_share();
        let eligible_fresh =
            eligible.iter().filter(|m| !m.current.exists).count() as f64 / eligible.len() as f64;
        if (eligible_fresh - model_fresh).abs() > FRESH_SHARE_TOL {
            return Selection::PoolNotRepresentative {
                eligible: eligible.len(),
                eligible_fresh_share: eligible_fresh,
                model_fresh_share: model_fresh,
            };
        }
        Selection::Matched(eligible.choose_multiple(rng, need).copied().collect())
    }

    /// Draw K−1 decoy profiles directly from the model, bypassing member maturity.
    ///
    /// This is the **idealized, fully-warmed pool** — every member already at target. It
    /// measures the defense's *ceiling* (what a mature pool achieves) separately from the
    /// operational cold-start question `select` models.
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
        ProfileModel::from_profiles([
            DestProfile::observed(900, Some(8_000_000), Some(100)),
            DestProfile::observed(500, Some(4_000_000), Some(50)),
            DestProfile::fresh(),
        ])
    }

    #[test]
    fn model_reproduces_the_fresh_share() {
        assert!((model3().fresh_share() - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn eligible_member_must_reach_target_age_and_txs() {
        let target = DestProfile::observed(900, Some(8_000_000), Some(100));
        let not_ready = PoolMember {
            index: 0,
            target,
            current: DestProfile::observed(10, Some(500), Some(5)),
        };
        let ready = PoolMember {
            index: 1,
            target,
            current: DestProfile::observed(950, Some(8_100_000), Some(90)),
        };
        assert!(!not_ready.is_eligible());
        assert!(ready.is_eligible());
    }

    #[test]
    fn fresh_target_member_is_always_eligible() {
        let m = PoolMember {
            index: 0,
            target: DestProfile::fresh(),
            current: DestProfile::fresh(),
        };
        assert!(m.is_eligible());
    }

    #[test]
    fn select_fails_closed_when_pool_is_cold() {
        let pool = WarmingPool::new(model3()); // no members warmed yet
        let mut rng = ChaCha20Rng::seed_from_u64(1);
        match pool.select(&DestProfile::fresh(), 8, &mut rng) {
            Selection::PoolTooCold {
                eligible: 0,
                needed: 7,
            } => {}
            other => panic!("cold pool must fail closed, got {other:?}"),
        }
    }

    /// A pool whose matured members reproduce model3's 1/3-fresh split matches.
    fn representative_matured_pool() -> WarmingPool {
        let hist = DestProfile::observed(900, Some(8_000_000), Some(100));
        let mut pool = WarmingPool::new(model3());
        // 16 history + 8 fresh, all matured => eligible fresh_share = 8/24 ≈ 0.33 ≈ model.
        for i in 0..16 {
            pool.members.push(PoolMember { index: i, target: hist, current: hist });
        }
        for i in 16..24 {
            pool.members.push(PoolMember {
                index: i,
                target: DestProfile::fresh(),
                current: DestProfile::fresh(),
            });
        }
        pool
    }

    #[test]
    fn select_matches_when_pool_reproduces_the_distribution() {
        let pool = representative_matured_pool();
        let mut rng = ChaCha20Rng::seed_from_u64(2);
        match pool.select(&DestProfile::observed(700, Some(6_000_000), Some(80)), 8, &mut rng) {
            Selection::Matched(d) => {
                assert_eq!(d.len(), 7);
                assert!(d.iter().all(|m| m.is_eligible()));
            }
            other => panic!("representative pool should match, got {other:?}"),
        }
    }

    #[test]
    fn select_fails_closed_when_only_fresh_members_matured() {
        // model3 is 1/3 fresh, but here only the fresh members have aged in (history
        // members still immature). Count is fine; the split is all-fresh => leaks =>
        // must fail closed on representativeness, not silently emit a leaking set.
        let hist = DestProfile::observed(900, Some(8_000_000), Some(100));
        let mut pool = WarmingPool::new(model3());
        for i in 0..8 {
            // fresh-target members: instantly eligible.
            pool.members.push(PoolMember {
                index: i,
                target: DestProfile::fresh(),
                current: DestProfile::fresh(),
            });
        }
        for i in 8..24 {
            // history-target members: still fresh (immature) => not eligible.
            pool.members.push(PoolMember { index: i, target: hist, current: DestProfile::fresh() });
        }
        let mut rng = ChaCha20Rng::seed_from_u64(3);
        match pool.select(&DestProfile::observed(700, Some(6_000_000), Some(80)), 8, &mut rng) {
            Selection::PoolNotRepresentative { eligible_fresh_share, .. } => {
                assert!((eligible_fresh_share - 1.0).abs() < 1e-9, "eligible are all fresh");
            }
            other => panic!("all-fresh eligible set must fail closed, got {other:?}"),
        }
    }
}
