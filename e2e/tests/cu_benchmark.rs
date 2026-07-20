//! Compute-unit benchmark for `execute_bundle`, K=2..=16, against the real program.
//!
//! LiteSVM already reports `compute_units_consumed` on every `send_transaction` result
//! (`litesvm::types::TransactionMetadata`) — the same dependency `sdk_e2e.rs` uses to
//! prove correctness, not a new one added just to measure cost. Mollusk-svm would do the
//! same measurement but is a second Solana-stack dependency to keep aligned with anchor
//! 0.31.1 + litesvm 0.6, for no capability this crate doesn't already have.
//!
//! Ceiling: `1_400_000` CU per transaction (`MAX_COMPUTE_UNIT_LIMIT`). A bundle that
//! neared it would need to justify a higher priority fee or a lower practical K; this
//! pins that it doesn't, with real numbers instead of an assumption.

use std::str::FromStr;

use litesvm::LiteSVM;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::Transaction,
};
use supersonic_sdk::{
    build_instruction, plan_bundle, DecoyConfig, DestProfile, PoolMember, ProfileModel, WarmingPool,
};

const SO_PATH: &str = "../target/deploy/supersonic_tx.so";
const PROGRAM_ID: &str = "D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be";
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
const SEED: [u8; 32] = [7u8; 32];
/// Solana's per-transaction compute budget ceiling.
const MAX_COMPUTE_UNIT_LIMIT: u64 = 1_400_000;

fn program_id() -> Pubkey {
    Pubkey::from_str(PROGRAM_ID).unwrap()
}

fn boot() -> (LiteSVM, Keypair) {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(program_id(), SO_PATH)
        .expect("load program .so — run `cargo build-sbf` first");
    let user = Keypair::new();
    svm.airdrop(&user.pubkey(), 100 * LAMPORTS_PER_SOL).unwrap();
    (svm, user)
}

fn warm_pool(n: u32) -> WarmingPool {
    let hist = DestProfile::observed(900, Some(8_000_000), Some(100));
    let model = ProfileModel::from_profiles((0..n).map(|_| hist));
    let mut pool = WarmingPool::new(model);
    for i in 0..n {
        pool.members.push(PoolMember {
            index: i,
            target: hist,
            current: hist,
        });
    }
    pool
}

fn cu_for_k(k: usize) -> u64 {
    let (mut svm, user) = boot();
    let plan = plan_bundle(
        &SEED,
        k as u64,
        Keypair::new().pubkey(),
        1_337_000,
        &warm_pool(20),
        k,
        DecoyConfig::default(),
    )
    .expect("warm pool should plan");
    let ix = build_instruction(program_id(), user.pubkey(), &plan);
    let bh = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&user.pubkey()), &[&user], bh);
    svm.send_transaction(tx)
        .expect("bundle must execute")
        .compute_units_consumed
}

#[test]
fn cu_scales_safely_with_k() {
    let ks: [usize; 5] = [2, 4, 8, 12, 16];
    let mut prev = 0u64;
    println!("K | compute units | headroom vs {MAX_COMPUTE_UNIT_LIMIT}");
    println!("--+----------------+------------------------");
    for &k in &ks {
        let cu = cu_for_k(k);
        println!(
            " {k:>2} | {cu:>14} | {:>6.1}%",
            100.0 * (1.0 - cu as f64 / MAX_COMPUTE_UNIT_LIMIT as f64)
        );

        assert!(
            cu < MAX_COMPUTE_UNIT_LIMIT,
            "K={k} used {cu} CU, over the {MAX_COMPUTE_UNIT_LIMIT} transaction ceiling"
        );
        assert!(
            cu >= prev,
            "K={k} used fewer CU ({cu}) than a smaller K ({prev}) — more legs should never be cheaper"
        );
        prev = cu;
    }
    // K=16 vs K=2: real numbers, not an assumed linear model. A few thousand CU per extra
    // leg (SPL system transfer + accounting) is expected; a runaway curve is a regression.
    let cu_min = cu_for_k(ks[0]);
    let cu_max = cu_for_k(*ks.last().unwrap());
    assert!(
        cu_max < cu_min * 10,
        "K=16 ({cu_max} CU) should not cost 10x K=2 ({cu_min} CU) — that would mean per-leg \
         cost grows with K instead of staying roughly constant"
    );
}
