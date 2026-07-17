//! # supersonic-dest-harness
//!
//! Measures the **destination-history channel** of `supersonic-tx` bundles — the
//! channel PR #1's threat model acknowledges and its harness explicitly does not
//! measure:
//!
//! > "The destination-history channel is a separate, acknowledged attack (defended
//! > operationally by pre-warming decoy addresses / a companion account-cooker), not
//! > something this harness claims to measure."
//!   — `harness/src/classifiers.rs`, PR #1
//!
//! This crate measures it, using PR #1's own metric (`advantage = accuracy − 1/K`)
//! and its train/test discipline, so the two numbers land in one comparable table.
//!
//! ## What makes the number credible
//!
//! The real leg's destination profile is **not synthesised**. It is sampled from an
//! empirical study of confirmed mainnet SOL transfers (`data/dest_study.jsonl`), so
//! the "real payee" population is the real one. The decoys are generated exactly as
//! PR #1 generates them — `derive_decoy_keypair` yields a fresh key, therefore a
//! profile with no history at all. The gap between those two populations is the
//! entire attack.

pub mod classifiers;
pub mod eval;
pub mod funding;
pub mod learned;
pub mod pool;
pub mod profile;

use rand::seq::SliceRandom;
use rand::Rng;
use serde::Deserialize;

use crate::pool::ProfileModel;
use crate::profile::{Bundle, DestProfile};

/// One row of the empirical mainnet study: a real transfer destination and its prior
/// footprint as of that transfer's slot. Produced by `scripts/dest_study.py`.
#[derive(Clone, Debug, Deserialize)]
pub struct StudyRow {
    pub dest: String,
    pub lamports: u64,
    pub slot: u64,
    pub prior_sigs: u32,
    pub had_history: bool,
    #[serde(default)]
    pub age_slots: Option<u64>,
    #[serde(default)]
    pub recency_slots: Option<u64>,
    /// True when the address's signature page hit the 1000-row RPC cap, i.e.
    /// `prior_sigs` is a floor rather than an exact count.
    #[serde(default)]
    pub page_full: bool,
}

impl StudyRow {
    pub fn to_profile(&self) -> DestProfile {
        DestProfile::observed(self.prior_sigs, self.age_slots, self.recency_slots)
    }
}

/// Parse the JSONL study. Malformed lines are surfaced, not skipped — a silently
/// dropped row biases the population we are trying to measure.
pub fn load_study(jsonl: &str) -> anyhow::Result<Vec<StudyRow>> {
    let mut out = Vec::new();
    for (i, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row: StudyRow =
            serde_json::from_str(line).map_err(|e| anyhow::anyhow!("study line {}: {e}", i + 1))?;
        out.push(row);
    }
    Ok(out)
}

/// Build one bundle the way PR #1 builds one, as an observer would see it.
///
/// * the real leg's destination profile is drawn from the empirical study;
/// * `k - 1` decoy destinations are fresh keys — `DestProfile::fresh()` — which is
///   what `derive_decoy_keypair` produces, with no history by construction;
/// * the real leg is placed at a uniformly-random index, as `plan_bundle` does.
pub fn sample_bundle<R: Rng>(study: &[StudyRow], k: usize, rng: &mut R) -> Bundle {
    debug_assert!(k >= 2);
    let real = study.choose(rng).expect("study must be non-empty");
    let mut profiles = vec![DestProfile::fresh(); k];
    let real_index = rng.gen_range(0..k);
    profiles[real_index] = real.to_profile();
    Bundle {
        profiles,
        real_index,
    }
}

/// Sample `n` bundles at anonymity-set size `k`.
pub fn sample_bundles<R: Rng>(study: &[StudyRow], k: usize, n: usize, rng: &mut R) -> Vec<Bundle> {
    (0..n).map(|_| sample_bundle(study, k, rng)).collect()
}

/// Split study rows into two halves deterministically, for train/test discipline.
/// The pool model is fit on `train`; real legs are drawn from `test`, so a measured
/// closure of the channel is generalization, not memorization of one file.
pub fn split<R: Rng>(study: &[StudyRow], rng: &mut R) -> (Vec<StudyRow>, Vec<StudyRow>) {
    let mut rows = study.to_vec();
    rows.shuffle(rng);
    let mid = rows.len() / 2;
    let test = rows.split_off(mid);
    (rows, test)
}

/// Build one **defended** bundle: the real leg's profile from `reals`, and `k-1` decoy
/// profiles drawn from the warmed pool `model` — i.i.d. from the reproduced
/// distribution, so the real is exchangeable with them. This is the counterpart to
/// [`sample_bundle`] (which uses fresh decoys, PR #1's construction).
pub fn sample_bundle_defended<R: Rng>(
    reals: &[StudyRow],
    model: &ProfileModel,
    k: usize,
    rng: &mut R,
) -> Bundle {
    debug_assert!(k >= 2);
    let real = reals
        .choose(rng)
        .expect("reals must be non-empty")
        .to_profile();
    let mut profiles: Vec<DestProfile> = (0..k - 1).map(|_| model.sample(rng)).collect();
    let real_index = rng.gen_range(0..k);
    profiles.insert(real_index, real);
    Bundle {
        profiles,
        real_index,
    }
}

/// Sample `n` defended bundles at anonymity-set size `k`.
pub fn sample_bundles_defended<R: Rng>(
    reals: &[StudyRow],
    model: &ProfileModel,
    k: usize,
    n: usize,
    rng: &mut R,
) -> Vec<Bundle> {
    (0..n)
        .map(|_| sample_bundle_defended(reals, model, k, rng))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifiers::DestClassifier;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    const STUDY: &str = r#"
{"dest":"A","lamports":1000000,"slot":100,"prior_sigs":974,"had_history":true,"age_slots":9000000,"recency_slots":120,"page_full":true}
{"dest":"B","lamports":2000000,"slot":100,"prior_sigs":0,"had_history":false}
{"dest":"C","lamports":3000000,"slot":100,"prior_sigs":9,"had_history":true,"age_slots":500,"recency_slots":10,"page_full":false}
"#;

    #[test]
    fn loads_rows_including_the_fresh_real_payee_case() {
        let rows = load_study(STUDY).unwrap();
        assert_eq!(rows.len(), 3);
        // The 8% case: a real payment to an address with no prior history. It is the
        // only thing standing between this attack and a perfect score, so it must
        // survive parsing.
        assert!(!rows[1].had_history);
        assert_eq!(rows[1].prior_sigs, 0);
        assert_eq!(rows[1].age_slots, None);
    }

    #[test]
    fn malformed_line_is_an_error_not_a_silent_drop() {
        assert!(load_study("{\"dest\":\"A\"}").is_err());
    }

    #[test]
    fn sampled_bundle_has_one_real_and_k_minus_one_fresh_decoys() {
        let study = load_study(STUDY).unwrap();
        let mut rng = ChaCha20Rng::seed_from_u64(1);
        let b = sample_bundle(&study, 8, &mut rng);
        assert_eq!(b.k(), 8);
        let fresh = b.profiles.iter().filter(|p| !p.exists).count();
        // 7 decoys are always fresh; the real leg is fresh only in the 8% case.
        assert!(
            fresh >= 7,
            "decoys must all be fresh, got {fresh} fresh of 8"
        );
    }

    #[test]
    fn advantage_tracks_the_share_of_real_payees_with_history() {
        // 2 of the 3 study rows have history. Over many bundles the exists attack
        // should land near that share, not near the 1/K baseline.
        let study = load_study(STUDY).unwrap();
        let mut rng = ChaCha20Rng::seed_from_u64(7);
        let bundles = sample_bundles(&study, 8, 4000, &mut rng);
        let s = eval::score(DestClassifier::Exists, &bundles);
        assert!(
            s.accuracy > 0.6,
            "expected ~2/3 (the share with history), got {:.3}",
            s.accuracy
        );
        assert!(
            s.advantage > 0.5,
            "advantage {:.3} should dwarf +0.013",
            s.advantage
        );
    }

    /// The day-2 gate, as an executable assertion: a warmed pool whose distribution
    /// reproduces the real-payee population drives the best destination attack to the
    /// 1/K baseline. Fresh decoys leak (high advantage); pool decoys do not.
    #[test]
    fn matched_pool_closes_the_channel() {
        use crate::eval::best_attack;
        use crate::pool::ProfileModel;

        // A study with realistic structure: ~63% have history at varied depths, ~37%
        // fresh — the same shape as the mainnet study, in miniature.
        let mut rows = Vec::new();
        for i in 0..600u64 {
            let line = if i % 100 < 37 {
                format!(
                    r#"{{"dest":"d{i}","lamports":1,"slot":10000,"prior_sigs":0,"had_history":false}}"#
                )
            } else {
                let sigs = 50 + (i % 900);
                let age = 100_000 + i * 1000;
                let rec = 10 + (i % 500);
                format!(
                    r#"{{"dest":"d{i}","lamports":1,"slot":10000,"prior_sigs":{sigs},"had_history":true,"age_slots":{age},"recency_slots":{rec}}}"#
                )
            };
            rows.push(line);
        }
        let study = load_study(&rows.join("\n")).unwrap();

        let mut rng = ChaCha20Rng::seed_from_u64(42);
        let (train, test) = split(&study, &mut rng);
        let model = ProfileModel::from_profiles(train.iter().map(|r| r.to_profile()));

        let k = 8;
        // Open: fresh decoys (PR #1's construction), real legs from the test split.
        let open_tr = sample_bundles(&test, k, 4000, &mut rng);
        let open_te = sample_bundles(&test, k, 4000, &mut rng);
        let open = best_attack(&open_tr, &open_te).unwrap();

        // Closed: pool decoys from the train-fit model, same real legs population.
        let cl_tr = sample_bundles_defended(&test, &model, k, 4000, &mut rng);
        let cl_te = sample_bundles_defended(&test, &model, k, 4000, &mut rng);
        let closed = best_attack(&cl_tr, &cl_te).unwrap();

        assert!(
            open.test.advantage > 0.4,
            "fresh decoys must leak, got {:+.3}",
            open.test.advantage
        );
        assert!(
            closed.test.advantage.abs() < 0.05,
            "matched pool must close the channel to ~0, got {:+.3}",
            closed.test.advantage
        );
    }
}
