//! Regression guard for the nonlinear-ensemble residual reported in `PROOF.md §2.3`.
//!
//! The forest's defended residual on the real study (~0.07-0.09 at the shipped
//! MAX_DEPTH=3) is larger than the linear learned adversary's (~0.01-0.03) — PROOF.md
//! explains why: with only 4 features, an ensemble that keeps finding more as depth
//! grows well past the point where depth already covers every feature interaction is
//! memorizing the finite, reused real-world sample, not learning a real gap. This test
//! pins that the *shipped* depth stays in the honestly-reported range — a future change
//! that let it drift back toward the open channel's advantage would be a regression.

use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

use supersonic_dest_harness::{
    forest::ForestAdversary,
    load_study,
    pool::{PoolMember, ProfileModel, WarmingPool},
    sample_bundles_via_select, split,
};

#[test]
fn forest_residual_stays_in_the_documented_range() {
    let raw = std::fs::read_to_string("data/dest_study.jsonl").expect("real study present");
    let study = load_study(&raw).expect("study parses");

    let mut split_rng = ChaCha20Rng::seed_from_u64(1 ^ 0x5D17);
    let (train, test) = split(&study, &mut split_rng);
    let model = ProfileModel::from_profiles(train.iter().map(|r| r.to_profile()));
    let mut pool = WarmingPool::new(model);
    for (i, r) in train.iter().enumerate() {
        let profile = r.to_profile();
        pool.members.push(PoolMember {
            index: i as u32,
            target: profile,
            current: profile,
        });
    }

    let k = 16;
    let mut rng = ChaCha20Rng::seed_from_u64(1 ^ (k as u64) << 32);
    let tr = sample_bundles_via_select(&test, &pool, k, 3000, &mut rng);
    let te = sample_bundles_via_select(&test, &pool, k, 3000, &mut rng);
    let residual = ForestAdversary::fit(&tr).advantage(&te).abs();

    assert!(
        residual < 0.15,
        "forest defended residual {residual:+.3} exceeds the documented 0.15 ceiling — \
         re-measure PROOF.md §2.3 before shipping, this may no longer be the memorization \
         effect it explains"
    );
    assert!(
        residual > 0.0,
        "forest defended residual is exactly zero — surprising given PROOF.md §2.3 \
         measured a real nonzero effect; re-check the pipeline hasn't changed"
    );
}
