//! A learned **union** adversary over the destination channel.
//!
//! The single-feature classifiers in `classifiers.rs` each read one signal (exists, txs,
//! age, recency). This is the stronger attacker DESIGN §3.1 promised: a conditional-logit
//! model that fits weights over **all** features jointly on the train split and picks the
//! highest-scoring leg on test. The point is robustness of the defense: if the warmed pool
//! closes the channel against *this*, it closes it against any linear combination of the
//! observable features — not merely against the best single classifier. On PR #1's
//! fresh-decoy construction it learns to lean on `exists` and recovers the full leak; on a
//! matched pool it finds no separating direction and collapses to the `1/K` baseline.

use crate::profile::{Bundle, DestProfile};

const NFEAT: usize = 4;
/// Sentinel "slots since last activity" for a never-active address: large, so the recency
/// feature never ranks a fresh leg as recently-active.
const NO_RECENCY: f64 = 10_000_000.0;

/// Per-leg feature vector, each component monotone in "looks like an established payee".
fn features(p: &DestProfile) -> [f64; NFEAT] {
    [
        p.exists as u8 as f64,
        (p.prior_sigs as f64).ln_1p(),
        (p.age_slots.unwrap_or(0) as f64).ln_1p(),
        // Smaller slots-since-last-activity = more recent = more payee-like, so negate.
        -(p.recency_slots.map(|r| r as f64).unwrap_or(NO_RECENCY)).ln_1p(),
    ]
}

fn dot(a: &[f64; NFEAT], b: &[f64; NFEAT]) -> f64 {
    (0..NFEAT).map(|j| a[j] * b[j]).sum()
}

/// Per-feature mean/scale fit on the train legs, so features on wildly different scales
/// (a 0/1 bit vs. a log-signature-count) each get a fair weight.
struct Standardizer {
    mean: [f64; NFEAT],
    inv_std: [f64; NFEAT],
}

impl Standardizer {
    fn fit(bundles: &[Bundle]) -> Self {
        let mut mean = [0.0; NFEAT];
        let mut n = 0.0;
        for b in bundles {
            for p in &b.profiles {
                let x = features(p);
                for j in 0..NFEAT {
                    mean[j] += x[j];
                }
                n += 1.0;
            }
        }
        if n > 0.0 {
            for m in &mut mean {
                *m /= n;
            }
        }
        let mut var = [0.0; NFEAT];
        for b in bundles {
            for p in &b.profiles {
                let x = features(p);
                for j in 0..NFEAT {
                    let d = x[j] - mean[j];
                    var[j] += d * d;
                }
            }
        }
        let mut inv_std = [1.0; NFEAT];
        if n > 0.0 {
            for j in 0..NFEAT {
                let sd = (var[j] / n).sqrt();
                // A constant feature (sd≈0) carries no signal; zero it out rather than
                // divide by ~0. On a matched pool `exists` is often constant — this is
                // what makes the learned attack correctly find nothing.
                inv_std[j] = if sd > 1e-9 { 1.0 / sd } else { 0.0 };
            }
        }
        Self { mean, inv_std }
    }

    fn apply(&self, p: &DestProfile) -> [f64; NFEAT] {
        let x = features(p);
        let mut z = [0.0; NFEAT];
        for j in 0..NFEAT {
            z[j] = (x[j] - self.mean[j]) * self.inv_std[j];
        }
        z
    }
}

/// A conditional-logit adversary: score each leg by `w·standardize(features)`, predict the
/// argmax. `w` is fit to maximize the softmax likelihood of the real leg on the train set.
pub struct LearnedAdversary {
    w: [f64; NFEAT],
    std: Standardizer,
}

impl LearnedAdversary {
    /// Fit by full-batch gradient ascent on the softmax log-likelihood. Deterministic (no
    /// RNG, fixed epoch/step) so the reported advantage is reproducible.
    pub fn fit(train: &[Bundle]) -> Self {
        let std = Standardizer::fit(train);
        let mut w = [0.0; NFEAT];
        let lr = 0.3;
        let epochs = 400;
        let m = train.len().max(1) as f64;
        for _ in 0..epochs {
            let mut grad = [0.0; NFEAT];
            for b in train {
                let zs: Vec<[f64; NFEAT]> = b.profiles.iter().map(|p| std.apply(p)).collect();
                let scores: Vec<f64> = zs.iter().map(|z| dot(&w, z)).collect();
                let mx = scores.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let exps: Vec<f64> = scores.iter().map(|s| (s - mx).exp()).collect();
                let sum: f64 = exps.iter().sum();
                // d(log p_real)/dw = x_real − E_p[x]. Ascend it.
                for j in 0..NFEAT {
                    let e_j: f64 = zs.iter().zip(&exps).map(|(z, e)| e / sum * z[j]).sum();
                    grad[j] += zs[b.real_index][j] - e_j;
                }
            }
            for j in 0..NFEAT {
                w[j] += lr * grad[j] / m;
            }
        }
        Self { w, std }
    }

    pub fn predict(&self, b: &Bundle) -> usize {
        let mut best = 0usize;
        let mut best_v = f64::NEG_INFINITY;
        for (i, p) in b.profiles.iter().enumerate() {
            let v = dot(&self.w, &self.std.apply(p));
            if v > best_v {
                best_v = v;
                best = i;
            }
        }
        best
    }

    /// Test-split advantage over the `1/K` baseline: `accuracy − 1/K`.
    pub fn advantage(&self, test: &[Bundle]) -> f64 {
        if test.is_empty() {
            return 0.0;
        }
        let k = test[0].k();
        let hits = test
            .iter()
            .filter(|b| self.predict(b) == b.real_index)
            .count();
        hits as f64 / test.len() as f64 - 1.0 / k as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr1_bundles(k: usize) -> Vec<Bundle> {
        // K−1 fresh decoys + one established real payee, real leg cycling through every
        // position so there's no positional shortcut to learn.
        (0..k)
            .map(|real_index| {
                let mut profiles = vec![DestProfile::fresh(); k];
                profiles[real_index] = DestProfile::observed(900, Some(9_000_000), Some(120));
                Bundle {
                    profiles,
                    real_index,
                }
            })
            .collect()
    }

    #[test]
    fn learns_the_open_channel() {
        // Against fresh-decoy bundles the union adversary recovers the full leak, same as
        // the single `exists` bit: advantage ~ 1 − 1/K.
        let k = 8;
        let bundles = pr1_bundles(k);
        let adv = LearnedAdversary::fit(&bundles);
        let a = adv.advantage(&bundles);
        assert!(
            a > 0.85,
            "learned adversary should crack the open channel, got {a:+.3}"
        );
    }

    #[test]
    fn finds_nothing_on_a_matched_pool() {
        // Every leg drawn from one established distribution: no feature separates the real
        // leg, so the learned direction is degenerate and accuracy falls to ~1/K.
        let k = 4;
        let matched: Vec<Bundle> = (0..k)
            .map(|real_index| Bundle {
                profiles: vec![DestProfile::observed(900, Some(8_000_000), Some(100)); k],
                real_index,
            })
            .collect();
        let adv = LearnedAdversary::fit(&matched);
        assert!(
            adv.advantage(&matched).abs() < 1e-9,
            "a fully matched pool must leak nothing, even to the union adversary"
        );
    }
}
