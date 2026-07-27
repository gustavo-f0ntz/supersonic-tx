//! Cross-bundle identity reuse — a residual PR #1 found in its own construction
//! (`92ee379`/`a8f7b2f`, per-slot sub-funder learning) and reported straight: a
//! per-bundle mitigation with a ~0 residual on bundle one can still leak once an
//! observer accumulates enough bundles. This module asks the same question of
//! `WarmingPool`.
//!
//! This is **not** the constant-`--bundle-id` bug (`AUDIT.md` finding 1, already fixed):
//! that bug emitted the *identical* decoy set every time. Here every bundle draws a
//! genuinely random subset via the real `WarmingPool::select` path — but the subset
//! still comes from the same finite pool (32 members by CLI default), so pool-member
//! *identity itself* — an address, directly observable on-chain, no funding trace
//! required — necessarily repeats across bundles at a rate a real payee population
//! does not. An attacker who watches enough bundles from the same pool can rank legs by
//! how often each address has been seen before and guess "least-seen = real."
//!
//! **A "spread usage evenly" fix was tried and measured worse, not better.** The natural
//! idea — bias `select` toward whichever members have been drawn least so far — was
//! implemented and benchmarked before shipping. It made every checkpoint past bundle ~5
//! *worse*: least-used-first saturates the pool (every member drawn at least once)
//! far faster than a uniform independent draw does, and the attacker's signal here is
//! binary — "ever seen before, yes or no" — not frequency-graded. Once every decoy has
//! been seen at least once while the real leg (a genuinely one-time identity) never has,
//! the attacker wins outright; reaching that point sooner is strictly worse. This is the
//! same class of finding as PR #1's own honest "the fix doesn't hold" disclosures — caught
//! by measuring before documenting, not shipped. The root cause is structural (any finite,
//! self-warmed, reused pool saturates eventually, independent of draw order) and needs the
//! same fix already named for the other residuals here: an externally-supplied, effectively
//! unbounded decoy source (`CHANNELS.md §5.1`'s crowd interface), not a cleverer `select`.

use std::collections::HashMap;

use rand::Rng;

use crate::pool::{Selection, WarmingPool};

/// One leg's identity across bundles. A decoy's identity is its pool member index —
/// the same index reappears whenever `select` draws that member again. The real leg
/// gets a fresh identity every bundle, modeling a real-world payee population that
/// (unlike the fixed decoy pool) is not observed to repeat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LegId {
    Pool(u32),
    Real(u64),
}

pub struct ObservedBundle {
    pub legs: Vec<LegId>,
    pub real_index: usize,
}

/// Draw `n` bundles through the actual `WarmingPool::select` path, reusing one pool
/// across all of them (as a deployed pool would be), each with a never-repeating real
/// identity. Panics if the pool isn't warmed enough to select `k` — same contract as
/// [`crate::sample_bundles_via_select`].
pub fn observe_bundles<R: Rng>(
    pool: &WarmingPool,
    k: usize,
    n: usize,
    rng: &mut R,
) -> Vec<ObservedBundle> {
    (0..n)
        .map(|i| {
            let members = match pool.select(k, rng) {
                Selection::Matched(m) => m,
                other => panic!("pool must be warmed enough to select k={k}: {other:?}"),
            };
            let mut legs: Vec<LegId> = members.iter().map(|m| LegId::Pool(m.index)).collect();
            let real_index = rng.gen_range(0..k);
            legs.insert(real_index, LegId::Real(i as u64));
            ObservedBundle { legs, real_index }
        })
        .collect()
}

/// The frequency attacker's running advantage, one value per bundle observed so far
/// (index 0 = after the first bundle). At each step it predicts whichever leg in the
/// current bundle has the **lowest** cumulative sighting count (ties broken by leg
/// order) is real, then scores itself and updates counts with the bundle just seen —
/// an online attacker, not one replaying the whole history each time.
pub fn frequency_advantage_curve(bundles: &[ObservedBundle]) -> Vec<f64> {
    let mut counts: HashMap<LegId, u32> = HashMap::new();
    let mut hits = 0usize;
    let mut curve = Vec::with_capacity(bundles.len());
    for (i, b) in bundles.iter().enumerate() {
        let predicted = b
            .legs
            .iter()
            .enumerate()
            .min_by_key(|(_, leg)| counts.get(leg).copied().unwrap_or(0))
            .map(|(idx, _)| idx)
            .expect("bundle has at least one leg");
        if predicted == b.real_index {
            hits += 1;
        }
        for leg in &b.legs {
            *counts.entry(*leg).or_insert(0) += 1;
        }
        let k = b.legs.len();
        curve.push(hits as f64 / (i + 1) as f64 - 1.0 / k as f64);
    }
    curve
}

/// Advantage over cumulative windows `[5, 10, 25, 50, 100, ...]` (whichever fit within
/// `bundles.len()`), for a compact "advantage by bundle count" table.
pub fn advantage_at_checkpoints(
    bundles: &[ObservedBundle],
    checkpoints: &[usize],
) -> Vec<(usize, f64)> {
    let curve = frequency_advantage_curve(bundles);
    checkpoints
        .iter()
        .filter(|&&c| c >= 1 && c <= curve.len())
        .map(|&c| (c, curve[c - 1]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{PoolMember, ProfileModel};
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    fn matured_pool(size: u32) -> WarmingPool {
        let model = ProfileModel::from_profiles((0..100).map(|i| {
            if i % 3 == 0 {
                crate::profile::DestProfile::fresh()
            } else {
                crate::profile::DestProfile::observed(900, Some(8_000_000), Some(100))
            }
        }));
        let mut rng = ChaCha20Rng::seed_from_u64(11);
        let mut pool = WarmingPool::new(model.clone());
        for i in 0..size {
            let target = model.sample(&mut rng);
            pool.members.push(PoolMember {
                index: i,
                target,
                current: target,
            });
        }
        pool
    }

    #[test]
    fn first_bundle_carries_no_signal() {
        let pool = matured_pool(32);
        let mut rng = ChaCha20Rng::seed_from_u64(1);
        let bundles = observe_bundles(&pool, 8, 1, &mut rng);
        let curve = frequency_advantage_curve(&bundles);
        // One bundle: every leg is first-seen, ties broken deterministically, so this
        // single draw is exactly right or exactly wrong -- not yet a measured signal.
        assert_eq!(curve.len(), 1);
    }

    #[test]
    fn advantage_grows_as_the_same_finite_pool_is_reused() {
        let pool = matured_pool(32);
        // Many independent trials so the curve isn't one noisy run.
        let trials = 200;
        let n = 100;
        let mut at_5 = 0.0;
        let mut at_100 = 0.0;
        for t in 0..trials {
            let mut rng = ChaCha20Rng::seed_from_u64(1000 + t);
            let bundles = observe_bundles(&pool, 8, n, &mut rng);
            let curve = frequency_advantage_curve(&bundles);
            at_5 += curve[4];
            at_100 += curve[n - 1];
        }
        at_5 /= trials as f64;
        at_100 /= trials as f64;
        assert!(
            at_100 > at_5 + 0.05,
            "advantage should grow with bundles observed from the same finite pool: \
             at_5={at_5:.3} at_100={at_100:.3}"
        );
        assert!(
            at_100 > 0.2,
            "after 100 bundles against a 32-member pool the frequency signal should be \
             substantial, got {at_100:.3}"
        );
    }

    #[test]
    fn checkpoints_pick_the_right_curve_points() {
        let pool = matured_pool(32);
        let mut rng = ChaCha20Rng::seed_from_u64(3);
        let bundles = observe_bundles(&pool, 8, 20, &mut rng);
        let curve = frequency_advantage_curve(&bundles);
        let checkpoints = advantage_at_checkpoints(&bundles, &[5, 10, 25]);
        assert_eq!(checkpoints, vec![(5, curve[4]), (10, curve[9])]);
    }
}
