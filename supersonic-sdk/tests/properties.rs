//! Property-based tests for `plan_bundle` (proptest).
//!
//! The unit tests in `lib.rs`/`amounts.rs`/`pool.rs` check specific examples. These
//! check the load-bearing invariants for *arbitrary* inputs — any master seed, any
//! bundle id, any real amount across five orders of magnitude, any anonymity set
//! 2..=16, any real destination. If the planner can violate one of these for *some*
//! input, proptest shrinks to the minimal counterexample. For a privacy tool whose
//! whole pitch is "the real leg is exactly one exchangeable sample, and every decoy is
//! recoverable," these are exactly the guarantees that must hold universally, not just
//! on the examples we happened to pick.
//!
//! All cases share one fully-matured, 32-member pool (`matured_pool`) — pool warming
//! is a real-time process (`pool.rs` docs), not something to fuzz per case; what these
//! tests vary is every input `plan_bundle` itself takes.

use proptest::prelude::*;
use solana_sdk::{pubkey::Pubkey, signer::Signer};
use supersonic_sdk::{
    build_instruction, derive_pool_keypair, plan_bundle, DecoyConfig, DestProfile, PoolMember,
    ProfileModel, WarmingPool,
};

/// 32 members, fully matured (`current == target`), profiles drawn from a model with no
/// fresh members — so `eligible_fresh_share` is trivially `0.0`, matching the model's
/// `fresh_share()` of `0.0` and always passing the representativeness gate regardless of
/// which member indices `select` happens to draw. Same construction as the SDK's own
/// module doctest and `composability-demo`'s `demo_pool`.
fn matured_pool() -> WarmingPool {
    let model = ProfileModel::from_profiles(
        (0..64u32)
            .map(|i| DestProfile::observed(40 + i, Some(120_000 + u64::from(i) * 900), Some(500))),
    );
    let mut pool = WarmingPool::new(model.clone());
    let mut rng = rand::thread_rng();
    for index in 0..32u32 {
        let target = model.sample(&mut rng);
        pool.members.push(PoolMember {
            index,
            target,
            current: target,
        });
    }
    pool
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// For any valid input, the plan is well-formed: exactly K legs, exactly one real
    /// leg carrying the exact intent, and every amount strictly positive.
    #[test]
    fn plan_is_well_formed(
        seed in any::<[u8; 32]>(),
        bundle_id in any::<u64>(),
        real_amount in 1u64..=1_000_000_000_000,
        k in 2usize..=16,
        dest_bytes in any::<[u8; 32]>(),
    ) {
        let pool = matured_pool();
        let real_dest = Pubkey::new_from_array(dest_bytes);
        prop_assume!(real_dest != solana_sdk::system_program::ID);
        let plan = plan_bundle(&seed, bundle_id, real_dest, real_amount, &pool, k, DecoyConfig::default())
            .expect("a matured 32-member pool must supply any k in 2..=16");

        prop_assert_eq!(plan.legs.len(), k);

        let reals: Vec<_> = plan.legs.iter().filter(|l| l.is_real).collect();
        prop_assert_eq!(reals.len(), 1, "exactly one real leg");
        prop_assert_eq!(reals[0].dest, real_dest);
        prop_assert_eq!(reals[0].amount, real_amount, "real amount is never distorted");
        prop_assert_eq!(plan.real_index, plan.legs.iter().position(|l| l.is_real).unwrap());

        prop_assert!(plan.legs.iter().all(|l| l.amount > 0), "no zero-value leg");
    }

    /// Every decoy destination is recoverable from the master seed via its pool index,
    /// and no two decoys in one bundle share a pool index — the invariant the recovery
    /// path (and the "no repeated decoy" anonymity-set guarantee) depends on.
    #[test]
    fn decoys_are_fully_recoverable(
        seed in any::<[u8; 32]>(),
        bundle_id in any::<u64>(),
        real_amount in 1u64..=1_000_000_000_000,
        k in 2usize..=16,
        dest_bytes in any::<[u8; 32]>(),
    ) {
        let pool = matured_pool();
        let real_dest = Pubkey::new_from_array(dest_bytes);
        prop_assume!(real_dest != solana_sdk::system_program::ID);
        let plan = plan_bundle(&seed, bundle_id, real_dest, real_amount, &pool, k, DecoyConfig::default()).unwrap();

        let mut seen = std::collections::HashSet::new();
        for leg in plan.legs.iter().filter(|l| !l.is_real) {
            let idx = leg.pool_index.expect("decoy has a pool index");
            prop_assert!(seen.insert(idx), "no repeated pool index within one bundle");
            let kp = derive_pool_keypair(&seed, idx);
            prop_assert_eq!(kp.pubkey(), leg.dest, "decoy dest recoverable from seed + pool index");
        }
        prop_assert_eq!(seen.len(), k - 1, "exactly k-1 distinct decoys");
    }

    /// Every decoy amount stays inside the plausible band (widened to include the real
    /// amount). This is the support-boundary guarantee: no decoy lands at an
    /// implausible size that would give itself away.
    #[test]
    fn decoys_stay_in_plausible_band(
        seed in any::<[u8; 32]>(),
        bundle_id in any::<u64>(),
        real_amount in 1u64..=1_000_000_000_000,
        k in 2usize..=16,
        dest_bytes in any::<[u8; 32]>(),
    ) {
        let pool = matured_pool();
        let real_dest = Pubkey::new_from_array(dest_bytes);
        prop_assume!(real_dest != solana_sdk::system_program::ID);
        let cfg = DecoyConfig::default();
        let plan = plan_bundle(&seed, bundle_id, real_dest, real_amount, &pool, k, cfg).unwrap();

        let lo = cfg.min_lamports.min(real_amount).max(1);
        let hi = cfg.max_lamports.max(real_amount);
        for leg in plan.legs.iter().filter(|l| !l.is_real) {
            prop_assert!(leg.amount >= lo && leg.amount <= hi,
                "decoy {} outside band [{}, {}]", leg.amount, lo, hi);
        }
    }

    /// **Structural indistinguishability, proven exactly (not statistically).** Every
    /// leg's destination account has the same role (writable, non-signer) and the same
    /// fixed 8-byte instruction-data width. An observer using shape / account-role /
    /// data-width features has literally zero bits to work with here — only the amount
    /// (PROOF.md) and the destination address (CHANNELS.md §3.2) carry any signal.
    #[test]
    fn instruction_is_structurally_uniform_across_legs(
        seed in any::<[u8; 32]>(),
        bundle_id in any::<u64>(),
        real_amount in 1u64..=1_000_000_000_000,
        k in 2usize..=16,
        dest_bytes in any::<[u8; 32]>(),
    ) {
        let pool = matured_pool();
        let real_dest = Pubkey::new_from_array(dest_bytes);
        prop_assume!(real_dest != solana_sdk::system_program::ID);
        let plan = plan_bundle(&seed, bundle_id, real_dest, real_amount, &pool, k, DecoyConfig::default()).unwrap();
        let ix = build_instruction(Pubkey::new_unique(), Pubkey::new_unique(), &plan);

        // Accounts are [user(signer,writable), system_program(readonly), then one
        // destination per leg]. Every destination must have the identical structural role.
        let dests = &ix.accounts[2..];
        prop_assert_eq!(dests.len(), k, "one destination account per leg");
        for m in dests {
            prop_assert!(m.is_writable, "every leg dest is writable");
            prop_assert!(!m.is_signer, "no leg dest is a signer");
        }
        // Instruction data = 8-byte discriminator + 4-byte Borsh vec length + k x 8-byte
        // legs. Each leg occupies exactly 8 bytes, so no leg is wider/narrower than another.
        prop_assert_eq!(ix.data.len(), 8 + 4 + k * 8, "every leg is a fixed 8-byte cell");
    }

    /// The plan is a pure function of (master_seed, bundle_id, intent, k, pool): identical
    /// inputs produce byte-identical bundles. This is what makes decoys recoverable and
    /// the whole system reproducible.
    #[test]
    fn plan_is_deterministic(
        seed in any::<[u8; 32]>(),
        bundle_id in any::<u64>(),
        real_amount in 1u64..=1_000_000_000_000,
        k in 2usize..=16,
        dest_bytes in any::<[u8; 32]>(),
    ) {
        let pool = matured_pool();
        let real_dest = Pubkey::new_from_array(dest_bytes);
        prop_assume!(real_dest != solana_sdk::system_program::ID);
        let a = plan_bundle(&seed, bundle_id, real_dest, real_amount, &pool, k, DecoyConfig::default()).unwrap();
        let b = plan_bundle(&seed, bundle_id, real_dest, real_amount, &pool, k, DecoyConfig::default()).unwrap();
        prop_assert_eq!(a.amounts(), b.amounts());
        prop_assert_eq!(a.destinations(), b.destinations());
        prop_assert_eq!(a.real_index, b.real_index);
    }
}
