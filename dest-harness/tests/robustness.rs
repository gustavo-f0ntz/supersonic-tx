//! Is the headline closure a lucky train/test split, or does it hold generally?
//!
//! `dest_advantage --seed 1` is the number in `PROOF.md`. This test reruns the exact
//! same pipeline (split real study -> fit pool on train -> draw decoys via the
//! deployed `WarmingPool::select` -> best_attack on held-out test) across ten
//! independent seeds and asserts the defended residual never exceeds the tolerance
//! already published for the single-seed number, at every K. One bad seed here would
//! mean the PROOF.md table cherry-picked its split; none does.

use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

use supersonic_dest_harness::{
    eval::best_attack,
    load_study,
    pool::{PoolMember, ProfileModel, WarmingPool},
    sample_bundles_via_select, split,
};

/// Same ceiling `select_path_closes_the_channel` (lib.rs) already uses for the
/// deployed-path closure — a residual above this would be a regression, not noise.
const RESIDUAL_CEILING: f64 = 0.05;
const SEEDS: [u64; 10] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
const KS: [usize; 4] = [2, 4, 8, 16];

#[test]
fn defended_residual_is_stable_across_seeds() {
    let raw = std::fs::read_to_string("data/dest_study.jsonl").expect("real study present");
    let study = load_study(&raw).expect("study parses");

    let mut worst = 0.0f64;
    let mut sum = 0.0f64;
    let mut count = 0usize;

    for &seed in &SEEDS {
        let mut split_rng = ChaCha20Rng::seed_from_u64(seed ^ 0x5D17);
        let (train, test) = split(&study, &mut split_rng);
        let model = ProfileModel::from_profiles(train.iter().map(|r| r.to_profile()));

        let mut pool = WarmingPool::new(model.clone());
        for (i, r) in train.iter().enumerate() {
            let profile = r.to_profile();
            pool.members.push(PoolMember {
                index: i as u32,
                target: profile,
                current: profile,
            });
        }

        for &k in &KS {
            let mut rng = ChaCha20Rng::seed_from_u64(seed ^ (k as u64) << 32);
            let tr = sample_bundles_via_select(&test, &pool, k, 4000, &mut rng);
            let te = sample_bundles_via_select(&test, &pool, k, 4000, &mut rng);
            let defended = best_attack(&tr, &te).expect("classifiers present");

            let residual = defended.test.advantage.abs();
            worst = worst.max(residual);
            sum += residual;
            count += 1;
            println!(
                "seed {seed:>2}, K={k:>2}: defended = {:+.3}",
                defended.test.advantage
            );

            assert!(
                residual < RESIDUAL_CEILING,
                "seed {seed}, K={k}: defended advantage {:+.3} exceeds the {RESIDUAL_CEILING} \
                 ceiling — the closure is seed-dependent, not a property of the pool",
                defended.test.advantage
            );
        }
    }

    println!(
        "{} seeds x {} K values: mean |residual| = {:.4}, worst = {:.4}",
        SEEDS.len(),
        KS.len(),
        sum / count as f64,
        worst
    );
}
