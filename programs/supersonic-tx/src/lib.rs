//! # supersonic-tx — on-chain atomic router
//!
//! A neutral, **non-custodial** executor for intent-ambiguous transfer bundles. It
//! moves the signer's own lamports to `K` destinations in one atomic transaction and
//! is deliberately **oblivious** to which leg is the user's real intent — every leg
//! takes the identical code path.
//!
//! The privacy engineering does not live here. It lives in the off-chain SDK, which
//! selects decoy destinations from a warmed pool whose history profile is
//! exchangeable with real payees'. This program's only jobs are the two guarantees a
//! caller cannot safely provide themselves:
//!
//! 1. **Atomicity** — all legs settle or the whole bundle reverts. A partial send that
//!    landed the real leg without its decoys would defeat the entire tool.
//! 2. **Non-custody** — the program never holds funds. There is no pool, escrow, or
//!    PDA that owns lamports; each leg moves value directly from the signer to a
//!    destination the signer supplied. This is what keeps the tool off the
//!    money-transmitter line: it never receives or forwards a third party's funds.
//!
//! Why a program at all, when a v0 transaction with N `SystemProgram::transfer`
//! instructions is already atomic? Two reasons, both about what a bare instruction
//! list cannot give: a **stable program id** other tools route through (the
//! composability the bounty asks for), and **enforced invariants** — a caller building
//! raw instructions can silently ship a malformed bundle (a zero leg, a self-send, a
//! single-leg "bundle" with no decoys) that leaks. Here those fail closed.

use anchor_lang::prelude::*;
use anchor_lang::system_program::{transfer, Transfer};

declare_id!("D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be");

/// Minimum legs per bundle.
///
/// A single-leg "bundle" (K=1) is a privacy no-op: it carries no decoys, yet routing
/// a plain transfer through *this* program still advertises "I use the ambiguity
/// tool." That is strictly worse than a normal transfer — it flags the user without
/// hiding anything. Rejecting K<2 forces every bundle to carry at least one decoy, so
/// the program is never used in a way that only incriminates its user. (PR #1 allows
/// K=1; this is a deliberate, pro-privacy tightening.)
pub const MIN_LEGS: usize = 2;

/// Maximum legs per bundle.
///
/// Bounded by the single-transaction envelope, not by compute. Account locks:
/// `signer(1) + system_program(1) + K destinations` = K+2; at K=16 that is 18, well
/// under the 64-lock cap. Transaction size is the tighter limit: ~18 account keys
/// (32B each) + instruction data (8B discriminator + 4B vec len + 8B per leg) ≈ 716B,
/// inside the 1232B packet without needing an Address Lookup Table. CU is a rounding
/// error (~150 CU per System transfer). 16 is therefore the largest anonymity set
/// that fits a bare transaction; larger K would force ALTs and buy little.
pub const MAX_LEGS: usize = 16;

#[program]
pub mod supersonic_tx {
    use super::*;

    /// Execute an atomic bundle of structurally-identical transfer legs.
    ///
    /// Each leg moves `amount` lamports from the signer to the destination at the same
    /// position in `remaining_accounts`. Real and decoy legs are byte-for-byte
    /// identical in shape; the program cannot and does not tell them apart.
    ///
    /// # Invariants (see CHANNELS.md §4)
    /// - **I3 Atomicity:** any failing leg reverts the whole bundle (Solana gives this
    ///   within a transaction; a partial exposure of the real leg is impossible).
    /// - **I4 Fail-closed:** a malformed leg (empty/oversized/mismatched bundle, zero
    ///   amount, self-send, insufficient funds) reverts everything, moving no funds.
    /// - **Uniformity:** every leg runs the identical code path; the program never
    ///   inspects or classifies a destination, because any on-chain "is this the
    ///   user's own?" check would brand decoys and leak exactly what we hide.
    pub fn execute_bundle<'info>(
        ctx: Context<'_, '_, '_, 'info, ExecuteBundle<'info>>,
        legs: Vec<Leg>,
    ) -> Result<()> {
        require!(legs.len() >= MIN_LEGS, SupersonicError::BundleTooSmall);
        require!(legs.len() <= MAX_LEGS, SupersonicError::TooManyLegs);

        let user = &ctx.accounts.user;
        let system_program = &ctx.accounts.system_program;
        let dests = ctx.remaining_accounts;

        // Structural uniformity: exactly one destination per leg, in leg order.
        require!(
            dests.len() == legs.len(),
            SupersonicError::AccountCountMismatch
        );

        for (leg, dest) in legs.iter().zip(dests.iter()) {
            // I4: a zero-value leg is a trivially-filterable decoy — reject.
            require!(leg.amount > 0, SupersonicError::ZeroAmount);
            // I4: a self-send is an economically pointless round-trip and a tell.
            require!(dest.key() != user.key(), SupersonicError::SelfDestination);

            // Non-custodial transfer: the signer's own lamports move directly to the
            // destination. Insufficient funds fail the CPI and, by I3, revert the
            // whole bundle. The program owns nothing at any point.
            transfer(
                CpiContext::new(
                    system_program.to_account_info(),
                    Transfer {
                        from: user.to_account_info(),
                        to: dest.to_account_info(),
                    },
                ),
                leg.amount,
            )?;
        }

        Ok(())
    }
}

/// A single leg of a bundle: just an amount. Destinations ride in
/// `remaining_accounts`, one per leg, so real and decoy legs are indistinguishable at
/// the program interface — the leg struct carries no field that could differ.
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Leg {
    /// Lamports moved by this leg to its paired destination.
    pub amount: u64,
}

#[derive(Accounts)]
pub struct ExecuteBundle<'info> {
    /// The bundle signer. Writable (lamports leave it) and the sole funding source —
    /// there is no other account the program could move value from.
    #[account(mut)]
    pub user: Signer<'info>,

    pub system_program: Program<'info, System>,
    // Destinations arrive as `remaining_accounts`: one writable, non-signer account
    // per leg, in leg order. Intentionally untyped and unconstrained so real and
    // decoy destinations are identical at the interface.
}

#[error_code]
pub enum SupersonicError {
    #[msg(
        "Bundle must contain at least MIN_LEGS legs (a decoy-free bundle only advertises tool use)"
    )]
    BundleTooSmall,
    #[msg("Bundle exceeds MAX_LEGS")]
    TooManyLegs,
    #[msg("Number of destination accounts must equal the number of legs")]
    AccountCountMismatch,
    #[msg("Leg amount must be greater than zero")]
    ZeroAmount,
    #[msg("Leg destination must not be the signer")]
    SelfDestination,
}
