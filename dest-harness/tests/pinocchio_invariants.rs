//! Invariant tests for the Pinocchio reimplementation of the router core
//! (`bench/pinocchio-router`), via Mollusk — the same 6 scenarios `e2e/tests/
//! program_invariants.rs` proves against the real Anchor `.so` (I3 atomicity + every
//! I4 fail-closed case: bundle size bounds, account-count mismatch, zero-amount,
//! self-destination, later-leg revert).
//!
//! This is what makes the Pinocchio side of `BENCHMARK.md` more than a size
//! comparison: both implementations are proven to uphold the identical invariants, not
//! just measured for binary size. Assertions are generic (`is_err()`/`is_ok()`), not
//! pinned to a named error, because Pinocchio has no framework-generated error enum
//! the way Anchor's `SupersonicError` does — see `bench/pinocchio-router/src/lib.rs`
//! for the encoding.
//!
//! Lives here (not `e2e/`) because `e2e` also depends on litesvm 0.6, whose transitive
//! graph collides with Mollusk's when both land in one crate — see this crate's
//! `Cargo.toml` comment.
//!
//! Build the Pinocchio program first:
//!   `cargo build-sbf --manifest-path bench/pinocchio-router/Cargo.toml`

use mollusk_svm::{result::Check, Mollusk};
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

const SBF_OUT_DIR: &str = "../bench/pinocchio-router/target/deploy";
const PROGRAM_NAME: &str = "supersonic_tx_pinocchio";
// Arbitrary 32-byte program label for Mollusk's registry — this program isn't deployed
// anywhere; unlike Anchor's `declare_id!`, a Pinocchio program doesn't embed or check an
// on-chain identity, so any distinct id works here.
const PROGRAM_ID: Pubkey = Pubkey::new_from_array([0x50; 32]);
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
const MAX_LEGS: usize = 16;

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

/// Build the instruction + funded-account set for a bundle. `payer_lamports` lets tests
/// exercise the insufficient-funds path. `dests.len()` need not equal `amounts.len()` —
/// that mismatch is itself one of the invariants under test.
fn bundle(
    payer: Pubkey,
    dests: &[Pubkey],
    amounts: &[u64],
    payer_lamports: u64,
) -> (Instruction, Vec<(Pubkey, Account)>) {
    let mut accounts = vec![AccountMeta::new(payer, true)];
    let mut funded = vec![(
        payer,
        Account::new(payer_lamports, 0, &solana_sdk_ids::system_program::id()),
    )];
    for d in dests {
        accounts.push(AccountMeta::new(*d, false));
        funded.push((
            *d,
            Account::new(0, 0, &solana_sdk_ids::system_program::id()),
        ));
    }
    // The program's own logic never reads this account (`Transfer::invoke()` only takes
    // `from`/`to`), but the runtime needs the CPI target present among this instruction's
    // accounts to resolve it (see bench/pinocchio-router's module doc).
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
        data: encode(amounts),
    };
    (ix, funded)
}

fn fresh_dests(n: usize) -> Vec<Pubkey> {
    (0..n)
        .map(|i| Pubkey::new_from_array([(i + 1) as u8; 32]))
        .collect()
}

fn balance_of(accounts: &[(Pubkey, Account)], key: &Pubkey) -> u64 {
    accounts
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, a)| a.lamports)
        .unwrap_or(0)
}

#[test]
fn k1_bundle_rejected() {
    let payer = Pubkey::new_from_array([200; 32]);
    let dests = fresh_dests(1);
    let (ix, accounts) = bundle(payer, &dests, &[LAMPORTS_PER_SOL], 10 * LAMPORTS_PER_SOL);
    assert!(
        mollusk()
            .process_instruction(&ix, &accounts)
            .program_result
            .is_err(),
        "single-leg bundle must be rejected (K < MIN_LEGS)"
    );
}

#[test]
fn oversized_bundle_rejected() {
    let payer = Pubkey::new_from_array([201; 32]);
    let dests = fresh_dests(MAX_LEGS + 1);
    let amounts = vec![1_000u64; MAX_LEGS + 1];
    let (ix, accounts) = bundle(payer, &dests, &amounts, 10 * LAMPORTS_PER_SOL);
    assert!(
        mollusk()
            .process_instruction(&ix, &accounts)
            .program_result
            .is_err(),
        "bundle over MAX_LEGS must be rejected"
    );
}

#[test]
fn account_count_mismatch_rejected() {
    let payer = Pubkey::new_from_array([202; 32]);
    let dests = fresh_dests(2); // two destinations for three legs
    let amounts = [1_000_000u64, 2_000_000, 3_000_000];
    let (ix, accounts) = bundle(payer, &dests, &amounts, 10 * LAMPORTS_PER_SOL);
    assert!(
        mollusk()
            .process_instruction(&ix, &accounts)
            .program_result
            .is_err(),
        "leg/destination count mismatch must be rejected"
    );
}

#[test]
fn zero_amount_leg_rejected() {
    let payer = Pubkey::new_from_array([203; 32]);
    let dests = fresh_dests(2);
    let amounts = [1_000_000u64, 0];
    let (ix, accounts) = bundle(payer, &dests, &amounts, 10 * LAMPORTS_PER_SOL);
    let result = mollusk().process_instruction(&ix, &accounts);
    assert!(
        result.program_result.is_err(),
        "zero-amount leg must be rejected"
    );
    for d in &dests {
        assert_eq!(
            balance_of(&result.resulting_accounts, d),
            0,
            "no leg lands on revert"
        );
    }
}

#[test]
fn self_send_leg_rejected() {
    let payer = Pubkey::new_from_array([204; 32]);
    let good = Pubkey::new_from_array([205; 32]);
    let dests = [good, payer];
    let amounts = [1_000_000u64, 1_000_000];
    let (ix, accounts) = bundle(payer, &dests, &amounts, 10 * LAMPORTS_PER_SOL);
    let result = mollusk().process_instruction(&ix, &accounts);
    assert!(
        result.program_result.is_err(),
        "self-destination leg must be rejected"
    );
    assert_eq!(
        balance_of(&result.resulting_accounts, &good),
        0,
        "valid sibling leg rolled back too (atomicity)"
    );
}

#[test]
fn later_leg_failure_reverts_earlier_legs() {
    // Leg 0 is a valid transfer that would execute; leg 1 asks for more than the
    // remaining balance and the CPI fails. Atomicity requires leg 0's already-applied
    // transfer to be rolled back to zero — the case the SDK can never build itself.
    let payer = Pubkey::new_from_array([206; 32]);
    let dests = fresh_dests(2);
    let amounts = [400_000_000u64, 900_000_000]; // 0.4 fits; 0.9 more does not, out of 1 SOL
    let (ix, accounts) = bundle(payer, &dests, &amounts, LAMPORTS_PER_SOL);
    let result = mollusk().process_instruction(&ix, &accounts);
    assert!(
        result.program_result.is_err(),
        "insufficient-funds later leg must fail the bundle"
    );
    assert_eq!(
        balance_of(&result.resulting_accounts, &dests[0]),
        0,
        "leg 0 transferred then rolled back — no partial landing exposes the real leg"
    );
}

#[test]
fn valid_bundle_succeeds_and_distributes() {
    let payer = Pubkey::new_from_array([207; 32]);
    let dests = fresh_dests(4);
    let amounts = [250_000_000u64, 1_337_000, 42_000_000, 90_000_000];
    let (ix, accounts) = bundle(payer, &dests, &amounts, 10 * LAMPORTS_PER_SOL);
    let result = mollusk().process_and_validate_instruction(&ix, &accounts, &[Check::success()]);
    for (dest, amt) in dests.iter().zip(amounts.iter()) {
        assert_eq!(
            balance_of(&result.resulting_accounts, dest),
            *amt,
            "each dest funded"
        );
    }
}
