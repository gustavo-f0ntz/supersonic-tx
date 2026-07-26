//! Compute-unit benchmark for the Pinocchio router (`bench/pinocchio-router`), K=2..16,
//! via Mollusk. See `BENCHMARK.md` for the comparison against the Anchor program's real
//! numbers (`e2e/tests/cu_benchmark.rs`, measured separately under LiteSVM — kept as two
//! honestly-labeled measurements rather than force-fitting both programs through one
//! harness, since `e2e` deliberately does not depend on Mollusk; see that crate's own
//! `cu_benchmark.rs` doc comment).
//!
//! Binary size / deploy rent is the load-bearing result of this benchmark, not CU — the
//! framework overhead Pinocchio removes is a fixed per-instruction cost, not proportional
//! to the K System-transfers this program actually does, so the CU gap should be roughly
//! constant across K, not a growing fraction. Measured below to confirm that, not to
//! headline it.

use mollusk_svm::{result::Check, Mollusk};
use mollusk_svm_bencher::MolluskComputeUnitBencher;
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

const SBF_OUT_DIR: &str = "../bench/pinocchio-router/target/deploy";
const PROGRAM_NAME: &str = "supersonic_tx_pinocchio";
const PROGRAM_ID: Pubkey = Pubkey::new_from_array([0x50; 32]);
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

fn mollusk() -> Mollusk {
    std::env::set_var("SBF_OUT_DIR", SBF_OUT_DIR);
    Mollusk::new(&PROGRAM_ID, PROGRAM_NAME)
}

fn encode(amounts: &[u64]) -> Vec<u8> {
    let mut data = vec![amounts.len() as u8];
    for a in amounts {
        data.extend_from_slice(&a.to_le_bytes());
    }
    data
}

fn bundle_ix_and_accounts(k: usize) -> (Instruction, Vec<(Pubkey, Account)>) {
    let payer = Pubkey::new_unique();
    let amounts: Vec<u64> = (0..k).map(|i| 1_337_000 + i as u64 * 1_000).collect();

    let mut accounts = vec![AccountMeta::new(payer, true)];
    let mut funded = vec![(
        payer,
        Account::new(
            10 * LAMPORTS_PER_SOL,
            0,
            &solana_sdk_ids::system_program::id(),
        ),
    )];
    for _ in 0..k {
        let dest = Pubkey::new_unique();
        accounts.push(AccountMeta::new(dest, false));
        funded.push((
            dest,
            Account::new(0, 0, &solana_sdk_ids::system_program::id()),
        ));
    }
    accounts.push(AccountMeta::new_readonly(
        solana_sdk_ids::system_program::id(),
        false,
    ));
    let mut system_program_account = Account::new(1, 0, &solana_sdk_ids::native_loader::id());
    system_program_account.executable = true;
    funded.push((solana_sdk_ids::system_program::id(), system_program_account));

    let ix = Instruction {
        program_id: PROGRAM_ID,
        accounts,
        data: encode(&amounts),
    };
    (ix, funded)
}

/// Sanity check before trusting any CU number: the bundle actually succeeds.
#[test]
fn bundle_executes_successfully_under_mollusk() {
    let (ix, accounts) = bundle_ix_and_accounts(8);
    mollusk().process_and_validate_instruction(&ix, &accounts, &[Check::success()]);
}

/// The CU-vs-K curve, written to `target/benches` (matching `BENCHMARK.md`'s
/// "reproduce" instructions).
#[test]
fn cu_scales_with_leg_count() {
    let (ix2, acc2) = bundle_ix_and_accounts(2);
    let (ix4, acc4) = bundle_ix_and_accounts(4);
    let (ix8, acc8) = bundle_ix_and_accounts(8);
    let (ix16, acc16) = bundle_ix_and_accounts(16);

    MolluskComputeUnitBencher::new(mollusk())
        .bench(("pinocchio_router_k2", &ix2, &acc2))
        .bench(("pinocchio_router_k4", &ix4, &acc4))
        .bench(("pinocchio_router_k8", &ix8, &acc8))
        .bench(("pinocchio_router_k16", &ix16, &acc16))
        .must_pass(true)
        .out_dir("../target/benches")
        .execute();
}
