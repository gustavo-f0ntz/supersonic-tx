//! A nonlinear ensemble adversary over the destination channel — same 4 features as
//! `learned.rs`'s conditional-logit, a strictly different hypothesis class.
//!
//! `LearnedAdversary` can only separate legs along one learned linear direction. If the
//! defended pool's residual were an artifact of that limitation — some nonlinear
//! combination of `(exists, prior_sigs, age, recency)` the logit can't express — a
//! stronger attacker would find it. This module is that check: a small ensemble of
//! randomized decision trees (extra-trees-style random cut points, bagged like a random
//! forest — built from scratch, not a port of scikit-learn's implementation) fit as a
//! real-vs-decoy classifier over individual leg profiles, then used to rank legs within
//! a bundle exactly as the logit is.
//!
//! Each tree, at every node, picks one random threshold per feature (Geurts et al.'s
//! extra-trees construction) and keeps whichever feature's split best separates
//! real-labeled from decoy-labeled examples by Gini impurity — so trees see thresholds
//! the logit never considers, and the forest averages many such shallow, randomized cuts.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;

use crate::learned::{features, NFEAT};
use crate::profile::{Bundle, DestProfile};

const N_TREES: usize = 100;
const MAX_DEPTH: u32 = 3;
const MIN_LEAF: usize = 10;

enum Node {
    Leaf {
        prob_real: f64,
    },
    Split {
        feature: usize,
        threshold: f64,
        left: Box<Node>,
        right: Box<Node>,
    },
}

fn gini(pos: usize, n: usize) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let p = pos as f64 / n as f64;
    2.0 * p * (1.0 - p)
}

fn fit_node(examples: &[([f64; NFEAT], bool)], depth: u32, rng: &mut ChaCha20Rng) -> Node {
    let n = examples.len();
    let pos = examples.iter().filter(|(_, y)| *y).count();

    if n < 2 * MIN_LEAF || depth >= MAX_DEPTH || pos == 0 || pos == n {
        return Node::Leaf {
            prob_real: pos as f64 / n as f64,
        };
    }

    let parent_gini = gini(pos, n);
    let mut best: Option<(usize, f64, f64)> = None; // (feature, threshold, gini decrease)

    for feat in 0..NFEAT {
        let (lo, hi) = examples
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), (x, _)| {
                (lo.min(x[feat]), hi.max(x[feat]))
            });
        if hi <= lo {
            continue; // constant feature at this node — no split possible
        }
        // Extra-trees' signature move: one random cut point per feature, not the
        // optimal one — the randomness is what makes trees in the forest disagree.
        let threshold = rng.gen_range(lo..hi);
        let (mut left_pos, mut left_n) = (0usize, 0usize);
        for (x, y) in examples {
            if x[feat] <= threshold {
                left_n += 1;
                if *y {
                    left_pos += 1;
                }
            }
        }
        let right_n = n - left_n;
        if left_n == 0 || right_n == 0 {
            continue;
        }
        let right_pos = pos - left_pos;
        let decrease = parent_gini
            - (left_n as f64 / n as f64) * gini(left_pos, left_n)
            - (right_n as f64 / n as f64) * gini(right_pos, right_n);
        if best.map(|(_, _, d)| decrease > d).unwrap_or(true) {
            best = Some((feat, threshold, decrease));
        }
    }

    let Some((feature, threshold, _)) = best else {
        return Node::Leaf {
            prob_real: pos as f64 / n as f64,
        };
    };

    let (left, right): (Vec<_>, Vec<_>) = examples
        .iter()
        .cloned()
        .partition(|(x, _)| x[feature] <= threshold);

    Node::Split {
        feature,
        threshold,
        left: Box::new(fit_node(&left, depth + 1, rng)),
        right: Box::new(fit_node(&right, depth + 1, rng)),
    }
}

impl Node {
    fn predict(&self, x: &[f64; NFEAT]) -> f64 {
        match self {
            Node::Leaf { prob_real } => *prob_real,
            Node::Split {
                feature,
                threshold,
                left,
                right,
            } => {
                if x[*feature] <= *threshold {
                    left.predict(x)
                } else {
                    right.predict(x)
                }
            }
        }
    }
}

/// An ensemble of `N_TREES` randomized trees, each bagged on a bootstrap resample of the
/// training legs — a real-vs-decoy classifier over single leg profiles.
pub struct ForestAdversary {
    trees: Vec<Node>,
}

impl ForestAdversary {
    /// Every leg of every training bundle becomes one labeled example (`is_real` as the
    /// label), independent of which bundle it came from — the forest learns "does this
    /// profile look like a real payee", not "which leg in this specific bundle."
    /// Deterministic (fixed seed) so the reported advantage is reproducible.
    pub fn fit(train: &[Bundle]) -> Self {
        let examples: Vec<([f64; NFEAT], bool)> = train
            .iter()
            .flat_map(|b| {
                b.profiles
                    .iter()
                    .enumerate()
                    .map(move |(i, p)| (features(p), i == b.real_index))
            })
            .collect();

        let mut seed_rng = ChaCha20Rng::seed_from_u64(0xF0_1E57);
        let trees = (0..N_TREES)
            .map(|_| {
                let mut tree_rng = ChaCha20Rng::seed_from_u64(seed_rng.gen());
                let bag: Vec<_> = (0..examples.len())
                    .map(|_| examples[tree_rng.gen_range(0..examples.len())])
                    .collect();
                fit_node(&bag, 0, &mut tree_rng)
            })
            .collect();
        Self { trees }
    }

    fn score(&self, p: &DestProfile) -> f64 {
        let x = features(p);
        self.trees.iter().map(|t| t.predict(&x)).sum::<f64>() / self.trees.len() as f64
    }

    pub fn predict(&self, b: &Bundle) -> usize {
        let mut best = 0usize;
        let mut best_v = f64::NEG_INFINITY;
        for (i, p) in b.profiles.iter().enumerate() {
            let v = self.score(p);
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
        // Same construction learned.rs's logit is tested against: a nonlinear ensemble
        // should crack the single-bit leak at least as completely as a linear one does.
        let k = 8;
        let bundles = pr1_bundles(k);
        let adv = ForestAdversary::fit(&bundles);
        let a = adv.advantage(&bundles);
        assert!(
            a > 0.85,
            "forest adversary should crack the open channel, got {a:+.3}"
        );
    }

    #[test]
    fn finds_nothing_on_a_matched_pool() {
        let k = 4;
        let matched: Vec<Bundle> = (0..k)
            .map(|real_index| Bundle {
                profiles: vec![DestProfile::observed(900, Some(8_000_000), Some(100)); k],
                real_index,
            })
            .collect();
        let adv = ForestAdversary::fit(&matched);
        assert!(
            adv.advantage(&matched).abs() < 1e-9,
            "a fully matched pool must leak nothing, even to a nonlinear ensemble"
        );
    }
}
