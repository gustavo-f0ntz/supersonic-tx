//! `supersonic` — the client CLI for the channel-complete bundle system.
//!
//! Four subcommands cover the lifecycle the design needs:
//!
//! * `warm`    — derive the pool's decoy addresses from your seed and record them, so you
//!               can fund and age them into a matched history profile.
//! * `plan`    — turn one real transfer into an intent-ambiguous bundle drawn from the
//!               warmed pool. **Fails closed** (non-zero exit) if the pool is too cold.
//! * `inspect` — show exactly what an on-chain observer sees for a planned bundle.
//! * `recover` — derive the pool addresses to sweep parked decoy funds back to your sinks.
//!
//! `send` (submitting the built transaction to an RPC) is deliberately out of this
//! offline CLI; the SDK's `build_instruction` produces the instruction and the litesvm
//! integration test executes it end-to-end against the program.

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use solana_sdk::{pubkey::Pubkey, signer::Signer};

use supersonic_sdk::{
    derive_pool_keypair, derive_sink_keypair, plan_bundle, DecoyConfig, DestProfile, PoolMember,
    ProfileModel, WarmingPool,
};

#[derive(Parser)]
#[command(name = "supersonic", version, about = "Channel-complete intent-ambiguous bundles")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Derive `count` warmed decoy addresses from the seed and write a pool file.
    Warm {
        /// 64-hex-char master seed (32 bytes). Keep it secret — it recovers decoys.
        #[arg(long)]
        seed: String,
        /// How many pool members to derive.
        #[arg(long, default_value_t = 32)]
        count: u32,
        /// Where to write the pool state.
        #[arg(long, default_value = "pool.json")]
        out: PathBuf,
        /// Mark every member already matured to its target (localnet/demo only — on
        /// mainnet maturity accrues over real time as you exercise the addresses).
        #[arg(long)]
        mature: bool,
    },
    /// Plan an intent-ambiguous bundle for one real transfer.
    Plan {
        #[arg(long)]
        seed: String,
        #[arg(long, default_value = "pool.json")]
        pool: PathBuf,
        /// Real destination (base58 pubkey).
        #[arg(long)]
        to: String,
        /// Real amount, in lamports.
        #[arg(long)]
        amount: u64,
        /// Anonymity-set size (total legs incl. the real one), 2..=16.
        #[arg(long)]
        k: usize,
        /// Per-bundle nonce.
        #[arg(long, default_value_t = 1)]
        bundle_id: u64,
        /// The real destination has no prior history (the ~37% fresh-payee case).
        #[arg(long)]
        real_fresh: bool,
        /// Optionally write the observer-view plan for `inspect`.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Show what an on-chain observer sees for a saved plan.
    Inspect {
        #[arg(long)]
        plan: PathBuf,
    },
    /// Derive the pool addresses (and their sinks) to sweep decoy funds back.
    Recover {
        #[arg(long)]
        seed: String,
        #[arg(long, default_value = "pool.json")]
        pool: PathBuf,
    },
}

/// The persisted warmed pool.
#[derive(Serialize, Deserialize)]
struct PoolFile {
    members: Vec<PoolMember>,
}

/// The observer-visible surface of a bundle — everything a chain watcher reads, and
/// nothing the operator must keep secret (no real index).
#[derive(Serialize, Deserialize)]
struct ObserverView {
    bundle_id: u64,
    amounts: Vec<u64>,
    destinations: Vec<String>,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Warm { seed, count, out, mature } => warm(&seed, count, &out, mature),
        Cmd::Plan { seed, pool, to, amount, k, bundle_id, real_fresh, out } => {
            plan(&seed, &pool, &to, amount, k, bundle_id, real_fresh, out.as_deref())
        }
        Cmd::Inspect { plan } => inspect(&plan),
        Cmd::Recover { seed, pool } => recover(&seed, &pool),
    }
}

fn warm(seed: &str, count: u32, out: &std::path::Path, mature: bool) -> Result<()> {
    let seed = parse_seed(seed)?;
    let members: Vec<PoolMember> = (0..count)
        .map(|index| {
            let target = representative_target(index);
            // Immature until aged; `--mature` collapses that for a localnet demo.
            let current = if mature { target } else { DestProfile::fresh() };
            PoolMember { index, target, current }
        })
        .collect();

    // Print the addresses the user must fund and exercise to warm the pool.
    println!("warmed {count} pool members (mature={mature}):");
    for m in &members {
        let pk = derive_pool_keypair(&seed, m.index).pubkey();
        let want = m.target.prior_sigs;
        println!("  [{:>3}] {pk}  target: {want} sigs", m.index);
    }
    let json = serde_json::to_string_pretty(&PoolFile { members })?;
    std::fs::write(out, json).with_context(|| format!("writing {}", out.display()))?;
    println!("pool state -> {}", out.display());
    if !mature {
        println!("(members start immature; fund+exercise them, then re-warm with --mature to plan on localnet)");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn plan(
    seed: &str,
    pool_path: &std::path::Path,
    to: &str,
    amount: u64,
    k: usize,
    bundle_id: u64,
    real_fresh: bool,
    out: Option<&std::path::Path>,
) -> Result<()> {
    let seed = parse_seed(seed)?;
    let real_dest = Pubkey::from_str(to).with_context(|| format!("bad --to pubkey: {to}"))?;
    let pool = load_pool(pool_path)?;

    // The real destination's profile: an observer reads it with one RPC call. `--real-fresh`
    // is the ~37% case (a genuinely new payee), which a matched pool covers too.
    let real_profile = if real_fresh {
        DestProfile::fresh()
    } else {
        representative_target(u32::MAX) // a "has history" real payee
    };

    let plan = plan_bundle(
        &seed,
        bundle_id,
        real_dest,
        amount,
        real_profile,
        &pool,
        k,
        DecoyConfig::default(),
    );

    let plan = match plan {
        Ok(p) => p,
        Err(e @ supersonic_sdk::SdkError::PoolTooCold { .. })
        | Err(e @ supersonic_sdk::SdkError::PoolNotRepresentative { .. }) => {
            // Fail closed, loudly, non-zero — never a leaking bundle.
            eprintln!("REFUSED: {e}");
            std::process::exit(1);
        }
        Err(e) => bail!(e),
    };

    // Observer view on stdout (amounts + destinations, in transaction order).
    println!("bundle {bundle_id}: K={k}, {} lamports moved", plan.total_moved());
    for (i, leg) in plan.legs.iter().enumerate() {
        println!("  leg {i}: {:>14} lamports -> {}", leg.amount, leg.dest);
    }
    // The real index is the operator's secret — to stderr, not stdout, so a piped
    // observer-view capture never contains it.
    eprintln!("(operator only) real leg is at index {}", plan.real_index);

    if let Some(path) = out {
        let view = ObserverView {
            bundle_id,
            amounts: plan.amounts(),
            destinations: plan.destinations().iter().map(|d| d.to_string()).collect(),
        };
        std::fs::write(path, serde_json::to_string_pretty(&view)?)
            .with_context(|| format!("writing {}", path.display()))?;
        eprintln!("observer-view plan -> {}", path.display());
    }
    Ok(())
}

fn inspect(plan_path: &std::path::Path) -> Result<()> {
    let raw = std::fs::read_to_string(plan_path)
        .with_context(|| format!("reading {}", plan_path.display()))?;
    let view: ObserverView = serde_json::from_str(&raw).context("parsing plan file")?;
    println!("what an observer sees for bundle {} (K={}):", view.bundle_id, view.amounts.len());
    for (i, (a, d)) in view.amounts.iter().zip(&view.destinations).enumerate() {
        println!("  leg {i}: {a:>14} lamports -> {d}");
    }
    println!(
        "the amount channel is exchangeable (log-normal + roundness); the destination \
         channel is closed by the warmed pool. The real leg is not recoverable from this view."
    );
    Ok(())
}

fn recover(seed: &str, pool_path: &std::path::Path) -> Result<()> {
    let seed = parse_seed(seed)?;
    let pool = load_pool(pool_path)?;
    println!("recovery addresses (sweep each pool member's parked funds to its sink):");
    for m in &pool.members {
        let member = derive_pool_keypair(&seed, m.index).pubkey();
        let sink = derive_sink_keypair(&seed, m.index).pubkey();
        println!("  [{:>3}] member {member} -> sink {sink}", m.index);
    }
    println!(
        "(dispersed recovery: each member sweeps to its own sink in a separate tx, avoiding \
         the consolidation star. Submitting the sweeps needs an RPC and live balances.)"
    );
    Ok(())
}

fn load_pool(path: &std::path::Path) -> Result<WarmingPool> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading pool {}", path.display()))?;
    let file: PoolFile = serde_json::from_str(&raw).context("parsing pool file")?;
    // The model reproduces the members' target distribution — same population the pool is
    // warmed to present.
    let model = ProfileModel::from_profiles(file.members.iter().map(|m| m.target));
    let mut pool = WarmingPool::new(model);
    pool.members = file.members;
    Ok(pool)
}

/// A representative target profile for pool member `index`, shaped like the §1.3 study:
/// ~37% fresh, the rest with history at varied depths. Deterministic in `index`, so
/// `warm` is reproducible. Production fits this from `dest-harness/data/dest_study.jsonl`
/// instead of this built-in stand-in.
fn representative_target(index: u32) -> DestProfile {
    // Hash the index so fresh/history members interleave: at any count ~37% are fresh,
    // matching §1.3. (A blocky `index % 100 < 37` would make small pools 100% fresh.)
    let h = index.wrapping_mul(2_654_435_761);
    if h % 100 < 37 {
        return DestProfile::fresh();
    }
    let sigs = 50 + (index % 900);
    let age = 100_000 + (index as u64) * 1_000;
    let recency = 10 + (index as u64 % 500);
    DestProfile::observed(sigs, Some(age), Some(recency))
}

/// Parse a 64-hex-char (32-byte) master seed.
fn parse_seed(s: &str) -> Result<[u8; 32]> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() != 64 {
        bail!("seed must be 64 hex chars (32 bytes), got {}", s.len());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
            .with_context(|| "seed is not valid hex")?;
    }
    Ok(out)
}
