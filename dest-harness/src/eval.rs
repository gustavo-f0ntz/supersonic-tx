//! Scoring: `advantage = accuracy − 1/K`, on a held-out split.
//!
//! Same discipline as PR #1's `harness/src/eval.rs`: the adversary picks its best
//! attack on the **train** split and is scored on **test**, so a number can't be
//! cherry-picked from noise. We keep the metric identical to PR #1's so the two
//! results sit in one table and are directly comparable — that comparability is the
//! whole point of the contribution.

use crate::classifiers::DestClassifier;
use crate::profile::Bundle;

#[derive(Clone, Copy, Debug)]
pub struct Score {
    pub accuracy: f64,
    /// `accuracy − 1/K`. Zero means the channel carries no signal; positive means it
    /// leaks. PR #1 reports +0.013 at K=8 on the amount channel.
    pub advantage: f64,
    pub n: usize,
}

/// Accuracy of one classifier over a set of bundles, and its advantage over the
/// `1/K` baseline. All bundles must share the same `K`.
pub fn score(c: DestClassifier, bundles: &[Bundle]) -> Score {
    if bundles.is_empty() {
        return Score {
            accuracy: 0.0,
            advantage: 0.0,
            n: 0,
        };
    }
    let k = bundles[0].k();
    debug_assert!(
        bundles.iter().all(|b| b.k() == k),
        "mixed K in one score set"
    );
    let hits = bundles
        .iter()
        .filter(|b| c.predict(&b.profiles) == b.real_index)
        .count();
    let accuracy = hits as f64 / bundles.len() as f64;
    Score {
        accuracy,
        advantage: accuracy - 1.0 / k as f64,
        n: bundles.len(),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BestAttack {
    pub classifier: DestClassifier,
    pub train: Score,
    pub test: Score,
}

/// Pick the strongest classifier on `train`, report its score on `test`.
///
/// Selecting and reporting on the same split would inflate the number by the
/// selection itself; this is the guard PR #1 uses and we match it.
pub fn best_attack(train: &[Bundle], test: &[Bundle]) -> Option<BestAttack> {
    DestClassifier::all()
        .iter()
        .map(|&c| (c, score(c, train)))
        .max_by(|a, b| a.1.advantage.partial_cmp(&b.1.advantage).unwrap())
        .map(|(c, train_score)| BestAttack {
            classifier: c,
            train: train_score,
            test: score(c, test),
        })
}

/// Wilson score interval for a binomial proportion — the honest error bar on an
/// accuracy estimate. Reported alongside every number so a reader can see how much
/// of the result is sample size.
pub fn wilson_ci(hits: usize, n: usize, z: f64) -> (f64, f64) {
    if n == 0 {
        return (0.0, 0.0);
    }
    let n_f = n as f64;
    let p = hits as f64 / n_f;
    let z2 = z * z;
    let denom = 1.0 + z2 / n_f;
    let centre = p + z2 / (2.0 * n_f);
    let margin = z * ((p * (1.0 - p) / n_f) + (z2 / (4.0 * n_f * n_f))).sqrt();
    (
        ((centre - margin) / denom).max(0.0),
        ((centre + margin) / denom).min(1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::DestProfile;

    fn bundle_pr1_style(k: usize, real_index: usize) -> Bundle {
        // PR #1's construction: K-1 fresh decoys + one established real payee.
        let mut profiles = vec![DestProfile::fresh(); k];
        profiles[real_index] = DestProfile::observed(974, Some(9_000_000), Some(120));
        Bundle {
            profiles,
            real_index,
        }
    }

    #[test]
    fn pr1_construction_leaks_completely_on_the_exists_bit() {
        // Every real leg established, every decoy fresh => the classifier is perfect
        // and advantage is 1 - 1/K, not the +0.013 the amount channel reports.
        let k = 8;
        let bundles: Vec<_> = (0..k).map(|i| bundle_pr1_style(k, i)).collect();
        let s = score(DestClassifier::Exists, &bundles);
        assert_eq!(s.accuracy, 1.0);
        assert!((s.advantage - (1.0 - 1.0 / k as f64)).abs() < 1e-9);
    }

    #[test]
    fn a_fully_matched_pool_drives_advantage_to_zero() {
        // Every leg established => the exists bit is constant => the classifier
        // degenerates to index 0 and scores exactly 1/K over uniform real placement.
        let k = 4;
        let bundles: Vec<_> = (0..k)
            .map(|real_index| Bundle {
                profiles: vec![DestProfile::observed(900, Some(8_000_000), Some(100)); k],
                real_index,
            })
            .collect();
        let s = score(DestClassifier::Exists, &bundles);
        assert!(
            (s.advantage).abs() < 1e-9,
            "matched pool should not leak, got {}",
            s.advantage
        );
    }

    #[test]
    fn wilson_brackets_the_point_estimate() {
        let (lo, hi) = wilson_ci(23, 25, 1.96);
        assert!(
            lo < 0.92 && 0.92 < hi,
            "CI [{lo:.3}, {hi:.3}] must bracket 23/25"
        );
        assert!(lo > 0.7, "n=25 at 92% should not admit anything below ~0.7");
    }

    #[test]
    fn wilson_narrows_as_n_grows() {
        let (lo_small, hi_small) = wilson_ci(23, 25, 1.96);
        let (lo_big, hi_big) = wilson_ci(920, 1000, 1.96);
        assert!(
            hi_big - lo_big < hi_small - lo_small,
            "n=1000 must be tighter than n=25"
        );
    }
}
