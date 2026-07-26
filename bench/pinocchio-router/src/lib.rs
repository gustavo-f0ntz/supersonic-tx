//! Pinocchio reimplementation of the `supersonic-tx` router core (`execute_bundle` in
//! `programs/supersonic-tx/src/lib.rs`), for an Anchor-vs-Pinocchio benchmark — see
//! `BENCHMARK.md`.
//!
//! Benchmark methodology (framework-overhead question, binary size as the load-bearing
//! metric, CU as secondary) follows the prior art of `Jmkoygg/supersonic-tx`'s own
//! `bench/pinocchio-router`, credited in `BENCHMARK.md`; this implementation is written
//! independently against *our* program's invariants, not copied from his.
//!
//! Same work as `execute_bundle`: for each leg, a System-Program transfer from the
//! signer to a destination, atomically, with the identical fail-closed invariants
//! (`CHANNELS.md §4`): bundle-size bounds, exact leg/destination count, no zero-amount
//! legs, no self-destination legs.
//!
//! Instruction data layout (a compact manual encoding — no Borsh/discriminator, since
//! this program has no framework to generate one):
//!   `[count: u8][count × u64 LE amounts]`
//! Accounts: `[0]` = payer/signer, `[1..1+count]` = destinations (in leg order),
//! `[1+count]` = System Program. The program's own logic never reads that last
//! account (`Transfer::invoke()` only takes `from`/`to`), but the runtime still needs
//! the CPI target present among the calling instruction's accounts to resolve it.

#![no_std]

use pinocchio::{entrypoint, error::ProgramError, AccountView, Address, ProgramResult};
use pinocchio_system::instructions::Transfer;

entrypoint!(process_instruction);
// no_std program: the entrypoint macro sets up the allocator; we only add a panic handler.
pinocchio::nostd_panic_handler!();

/// Minimum legs per bundle — matches `programs/supersonic-tx/src/lib.rs::MIN_LEGS`. A
/// single-leg "bundle" carries no decoys: it advertises tool use without hiding
/// anything, strictly worse than a plain transfer.
pub const MIN_LEGS: usize = 2;

/// Maximum legs per bundle — matches `programs/supersonic-tx/src/lib.rs::MAX_LEGS`
/// (account-lock bound, see that file's doc comment for the derivation).
pub const MAX_LEGS: usize = 16;

pub fn process_instruction(
    _program_id: &Address,
    accounts: &mut [AccountView],
    data: &[u8],
) -> ProgramResult {
    if data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    let count = data[0] as usize;
    // I4 (fail-closed): bundle-size bounds, mirroring `BundleTooSmall`/`TooManyLegs`.
    // Deliberately not `!(MIN_LEGS..=MAX_LEGS).contains(&count)` (clippy's own
    // suggestion here): measured on the real .so, that version nearly doubles this
    // binary (5,672 -> 10,736 bytes) under this no_std/BPF/LTO build — RangeInclusive's
    // trait-based check doesn't fold down as well as a raw comparison here. Binary size
    // is this crate's entire reason to exist (BENCHMARK.md Result 1), so the lint loses.
    #[allow(clippy::manual_range_contains)]
    if count < MIN_LEGS || count > MAX_LEGS {
        return Err(ProgramError::InvalidInstructionData);
    }
    if data.len() != 1 + count * 8 {
        return Err(ProgramError::InvalidInstructionData);
    }
    // Structural uniformity, mirroring `AccountCountMismatch`: exactly one destination
    // per leg, not just "at least" — extra accounts are rejected too. `+1` for the
    // trailing System Program account the CPI target resolution needs (see module doc).
    if accounts.len() != 2 + count {
        return Err(ProgramError::NotEnoughAccountKeys);
    }

    let accounts: &[AccountView] = accounts;
    let payer = &accounts[0];
    if !payer.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    // Defense in depth: the runtime already refuses a system transfer whose
    // `from`/`to` aren't writable, so this can't currently be bypassed — but unlike
    // Anchor, this program has no framework asserting it on our behalf.
    if !payer.is_writable() {
        return Err(ProgramError::InvalidAccountData);
    }

    for i in 0..count {
        let off = 1 + i * 8;
        let mut amt = [0u8; 8];
        amt.copy_from_slice(&data[off..off + 8]);
        let amount = u64::from_le_bytes(amt);
        // I4: mirroring `ZeroAmount` — a zero-value leg is a trivially-filterable decoy.
        if amount == 0 {
            return Err(ProgramError::InvalidInstructionData);
        }
        let dest = &accounts[1 + i];
        // I4: mirroring `SelfDestination` — a self-send is an economically pointless
        // round-trip and a tell.
        if dest.address() == payer.address() {
            return Err(ProgramError::InvalidInstructionData);
        }
        if !dest.is_writable() {
            return Err(ProgramError::InvalidAccountData);
        }
        // I3 (atomicity): any failing leg (e.g. insufficient funds on a later leg)
        // aborts the instruction, and Solana's transaction-level atomicity reverts
        // every already-applied transfer in this same instruction with it.
        Transfer {
            from: payer,
            to: dest,
            lamports: amount,
        }
        .invoke()?;
    }

    Ok(())
}
