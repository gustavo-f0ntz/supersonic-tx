//! An independent tool that casts through `supersonic-tx` using *only* the published
//! `supersonic-sdk` crate — nothing in this binary reaches into the CLI, the program's
//! source, or any private helper. It stands in for what the bounty's composability
//! requirement asks for: "another tool, including account-cooker, can cast through it."
//!
//! `account-cooker`'s public code (as of this writing) is a `[[bin]]`-only scaffold with
//! no library target to depend on yet, so this demo plays the caller's role itself rather
//! than importing it — but every line below is exactly what such a caller would write:
//! build a warmed pool, call `plan_bundle`, get back a plain `Instruction`, sign and send
//! it like any other Solana transaction. See `COMPOSABILITY.md` for the real devnet
//! signature this produced and what it does and does not prove.

use anyhow::{bail, Context, Result};
use clap::Parser;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{read_keypair_file, Signer},
    transaction::Transaction,
};
use std::str::FromStr;

use supersonic_sdk::{
    build_instruction, plan_bundle, DecoyConfig, DestProfile, PoolMember, ProfileModel, WarmingPool,
};

const PROGRAM_ID: &str = "D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be";

#[derive(Parser)]
#[command(about = "External-caller composability proof for supersonic-tx")]
struct Args {
    /// Real destination (base58 pubkey) — the one payment this bundle actually makes.
    #[arg(long)]
    to: String,
    /// Real amount, in lamports.
    #[arg(long)]
    amount: u64,
    /// Anonymity-set size (total legs incl. the real one).
    #[arg(long, default_value_t = 4)]
    k: usize,
    /// Funding signer's keypair file — pays every leg.
    #[arg(long)]
    keypair: std::path::PathBuf,
    /// RPC endpoint. No default: a mainnet URL is never hit by accident.
    #[arg(long)]
    rpc: String,
    /// Actually submit. Without it, the bundle is only simulated.
    #[arg(long)]
    broadcast: bool,
}

/// Builds a pool matured on the spot, exactly as the SDK's own module doctest does — a
/// stand-in for what a real caller gets from months of on-chain aging (or, per the
/// pitch in `README.md`, from an `account-cooker` agent's own history once one exists to
/// depend on). Not a claim that this pool is itself warmed against a real observer.
fn demo_pool() -> WarmingPool {
    let model = ProfileModel::from_profiles(
        (0..64u32)
            .map(|i| DestProfile::observed(40 + i, Some(120_000 + u64::from(i) * 900), Some(500))),
    );
    let mut pool = WarmingPool::new(model.clone());
    for index in 0..32u32 {
        let target = model.sample(&mut rand::thread_rng());
        pool.members.push(PoolMember {
            index,
            target,
            current: target,
        });
    }
    pool
}

fn main() -> Result<()> {
    let args = Args::parse();
    let program_id = Pubkey::from_str(PROGRAM_ID).expect("valid program id");
    let real_dest =
        Pubkey::from_str(&args.to).with_context(|| format!("bad --to pubkey: {}", args.to))?;
    let signer = read_keypair_file(&args.keypair)
        .map_err(|e| anyhow::anyhow!("reading keypair {}: {e}", args.keypair.display()))?;

    let pool = demo_pool();
    let plan = match plan_bundle(
        &[9u8; 32],
        1,
        real_dest,
        args.amount,
        &pool,
        args.k,
        DecoyConfig::default(),
    ) {
        Ok(p) => p,
        Err(e) => bail!("plan_bundle refused: {e}"),
    };

    println!(
        "composability-demo: bundle K={}, {} lamports moved, via supersonic-sdk only",
        args.k,
        plan.total_moved()
    );
    for (i, leg) in plan.legs.iter().enumerate() {
        println!("  leg {i}: {:>14} lamports -> {}", leg.amount, leg.dest);
    }

    let client = RpcClient::new(args.rpc.clone());
    let ix = build_instruction(program_id, signer.pubkey(), &plan);
    let blockhash = client
        .get_latest_blockhash()
        .with_context(|| format!("fetching blockhash from {}", args.rpc))?;
    let tx =
        Transaction::new_signed_with_payer(&[ix], Some(&signer.pubkey()), &[&signer], blockhash);

    if !args.broadcast {
        let sim = client
            .simulate_transaction(&tx)
            .with_context(|| "simulating bundle")?;
        match sim.value.err {
            None => println!("\nSIMULATED OK. Re-run with --broadcast to submit."),
            Some(err) => bail!("simulation failed: {err:?}\nlogs: {:#?}", sim.value.logs),
        }
        return Ok(());
    }

    let sig = client
        .send_and_confirm_transaction(&tx)
        .with_context(|| "submitting bundle")?;
    println!("\nBROADCAST — signature: {sig}");
    Ok(())
}
