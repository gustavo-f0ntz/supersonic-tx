//! Program-level invariant tests: the fail-closed guarantees (I4) and atomicity (I3) the
//! program's doc comment promises, proven against the real `.so` in LiteSVM.
//!
//! The seam test (`sdk_e2e.rs`) drives the happy path through the SDK. These bypass the
//! SDK and hand-build instructions that *violate* each invariant, so the program's own
//! `require!`s are what's under test — every `SupersonicError` variant, plus the case the
//! SDK can never produce: a later leg failing after an earlier leg already transferred,
//! which must roll the earlier transfer back.
//!
//! Build the program first:
//!   `cargo build-sbf --manifest-path programs/supersonic-tx/Cargo.toml`

use std::str::FromStr;

use litesvm::LiteSVM;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_program,
    transaction::Transaction,
};
use supersonic_sdk::execute_bundle_data;

const SO_PATH: &str = "../target/deploy/supersonic_tx.so";
const PROGRAM_ID: &str = "D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be";
const SOL: u64 = 1_000_000_000;

fn program_id() -> Pubkey {
    Pubkey::from_str(PROGRAM_ID).unwrap()
}

fn boot(airdrop: u64) -> (LiteSVM, Keypair) {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(program_id(), SO_PATH)
        .expect("load program .so — run `cargo build-sbf` first");
    let user = Keypair::new();
    svm.airdrop(&user.pubkey(), airdrop).unwrap();
    (svm, user)
}

/// Hand-build the `execute_bundle` instruction: signer + system_program, then one writable
/// non-signer destination per entry in `dests` (which need not match `amounts.len()` — that
/// mismatch is itself one of the invariants under test).
fn raw_ix(user: Pubkey, dests: &[Pubkey], amounts: &[u64]) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new(user, true),
        AccountMeta::new_readonly(system_program::ID, false),
    ];
    for d in dests {
        accounts.push(AccountMeta::new(*d, false));
    }
    Instruction {
        program_id: program_id(),
        accounts,
        data: execute_bundle_data(amounts),
    }
}

fn submit(svm: &mut LiteSVM, user: &Keypair, ix: Instruction) -> litesvm::types::TransactionResult {
    let bh = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&user.pubkey()), &[user], bh);
    svm.send_transaction(tx)
}

/// Assert the bundle reverts and the named Anchor error appears in the program logs, so the
/// test pins the *specific* invariant that fired, not merely "something failed".
fn expect_error(svm: &mut LiteSVM, user: &Keypair, ix: Instruction, error_name: &str) {
    let logs = submit(svm, user, ix)
        .expect_err("bundle must revert")
        .meta
        .logs;
    assert!(
        logs.iter().any(|l| l.contains(error_name)),
        "expected `{error_name}` in program logs, got: {logs:#?}"
    );
}

fn fresh_dests(n: usize) -> Vec<Pubkey> {
    (0..n).map(|_| Keypair::new().pubkey()).collect()
}

#[test]
fn k1_bundle_rejected() {
    // A single-leg "bundle" carries no decoy — it only advertises tool use. K>=2 enforced.
    let (mut svm, user) = boot(10 * SOL);
    let dests = fresh_dests(1);
    expect_error(&mut svm, &user, raw_ix(user.pubkey(), &dests, &[SOL]), "BundleTooSmall");
}

#[test]
fn oversized_bundle_rejected() {
    // K=17 exceeds MAX_LEGS=16 (the single-transaction envelope). Checked before anything.
    let (mut svm, user) = boot(100 * SOL);
    let dests = fresh_dests(17);
    let amounts = vec![1_000_000u64; 17];
    expect_error(&mut svm, &user, raw_ix(user.pubkey(), &dests, &amounts), "TooManyLegs");
}

#[test]
fn account_count_mismatch_rejected() {
    // Three legs declared, two destination accounts supplied: the 1:1 leg<->dest pairing
    // the uniformity guarantee depends on is broken. Reject.
    let (mut svm, user) = boot(10 * SOL);
    let dests = fresh_dests(2);
    let amounts = [1_000_000u64, 2_000_000, 3_000_000];
    expect_error(&mut svm, &user, raw_ix(user.pubkey(), &dests, &amounts), "AccountCountMismatch");
}

#[test]
fn zero_amount_leg_rejected() {
    // A zero-value leg is a trivially-filterable decoy. Reject, and move nothing.
    let (mut svm, user) = boot(10 * SOL);
    let dests = fresh_dests(2);
    let amounts = [1_000_000u64, 0];
    expect_error(&mut svm, &user, raw_ix(user.pubkey(), &dests, &amounts), "ZeroAmount");
    for d in &dests {
        assert_eq!(svm.get_balance(d).unwrap_or(0), 0, "no leg lands on revert");
    }
}

#[test]
fn self_send_leg_rejected() {
    // A leg paying the signer is an economic no-op and a tell. Reject, and move nothing —
    // including the sibling leg that on its own would have been valid (atomicity).
    let (mut svm, user) = boot(10 * SOL);
    let good = Keypair::new().pubkey();
    let dests = [good, user.pubkey()];
    let amounts = [1_000_000u64, 1_000_000];
    expect_error(&mut svm, &user, raw_ix(user.pubkey(), &dests, &amounts), "SelfDestination");
    assert_eq!(svm.get_balance(&good).unwrap_or(0), 0, "valid sibling leg rolled back too");
}

#[test]
fn later_leg_failure_reverts_earlier_legs() {
    // The case the SDK can never build: leg 0 is a valid transfer that *executes*, then leg
    // 1 asks for more than the remaining balance and the CPI fails. I3 requires the whole
    // bundle to revert, so leg 0's already-applied transfer must be rolled back to zero.
    let (mut svm, user) = boot(1 * SOL);
    let dests = fresh_dests(2);
    let amounts = [400_000_000u64, 900_000_000]; // 0.4 fits; 0.9 more does not
    assert!(
        submit(&mut svm, &user, raw_ix(user.pubkey(), &dests, &amounts)).is_err(),
        "insufficient-funds later leg must fail the bundle"
    );
    assert_eq!(
        svm.get_balance(&dests[0]).unwrap_or(0),
        0,
        "leg 0 transferred then rolled back — no partial landing exposes the real leg"
    );
}
