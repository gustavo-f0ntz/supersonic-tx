//! `supersonic` — the client CLI for the channel-complete bundle system.
//!
//! Subcommands cover the lifecycle the design needs:
//!
//! * `warm`    — derive the pool's decoy addresses from your seed and record them, so you
//!               can fund and age them into a matched history profile.
//! * `plan`    — turn one real transfer into an intent-ambiguous bundle drawn from the
//!               warmed pool. **Fails closed** (non-zero exit) if the pool is too cold.
//! * `inspect` — show exactly what an on-chain observer sees for a planned bundle.
//! * `recover` — derive the pool addresses to sweep parked decoy funds back to your sinks.
//! * `send`    — plan and submit to an RPC. **Simulates by default**; `--broadcast` to
//!               actually move funds. The program must be deployed on the target cluster.
//!
//! `warm`/`plan`/`inspect`/`recover` are fully offline; only `send` touches the network.

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    hash::Hash, pubkey::Pubkey, signature::read_keypair_file, signature::Keypair, signer::Signer,
    transaction::Transaction,
};

use supersonic_sdk::{
    build_instruction, derive_pool_keypair, derive_sink_keypair, plan_bundle, BundlePlan,
    DecoyConfig, DestProfile, PoolMember, ProfileModel, WarmingPool,
};

/// The deployed program id (matches `declare_id!` and `PROGRAM_ID.txt`). The program must
/// be deployed on the cluster `--rpc` points at — localnet/devnet in practice.
const PROGRAM_ID: &str = "D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be";

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
    /// Plan a bundle and submit it to an RPC. **Simulates by default** — pass `--broadcast`
    /// to actually move funds. Same planning inputs as `plan`, so the real destination
    /// stays in the signing path and is never written to a shared file.
    Send {
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
        #[arg(long)]
        k: usize,
        #[arg(long, default_value_t = 1)]
        bundle_id: u64,
        /// The funding signer's keypair file (the wallet that pays every leg).
        #[arg(long)]
        keypair: PathBuf,
        /// RPC endpoint. Required — no default, so a mainnet URL is never hit by accident.
        /// The program must be deployed on this cluster (localnet/devnet in practice).
        #[arg(long)]
        rpc: String,
        /// Actually submit. Without it, the bundle is only simulated and nothing is sent.
        #[arg(long)]
        broadcast: bool,
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
        Cmd::Plan { seed, pool, to, amount, k, bundle_id, out } => {
            plan(&seed, &pool, &to, amount, k, bundle_id, out.as_deref())
        }
        Cmd::Inspect { plan } => inspect(&plan),
        Cmd::Recover { seed, pool } => recover(&seed, &pool),
        Cmd::Send { seed, pool, to, amount, k, bundle_id, keypair, rpc, broadcast } => {
            send(&seed, &pool, &to, amount, k, bundle_id, &keypair, &rpc, broadcast)
        }
    }
}

/// Build the signed bundle transaction from a plan. Pure (no network), so it is unit-
/// testable and the network path in `send` is a thin wrapper over it.
fn build_bundle_tx(
    program_id: Pubkey,
    signer: &Keypair,
    plan: &BundlePlan,
    blockhash: Hash,
) -> Transaction {
    let ix = build_instruction(program_id, signer.pubkey(), plan);
    Transaction::new_signed_with_payer(&[ix], Some(&signer.pubkey()), &[signer], blockhash)
}

#[allow(clippy::too_many_arguments)]
fn send(
    seed: &str,
    pool_path: &std::path::Path,
    to: &str,
    amount: u64,
    k: usize,
    bundle_id: u64,
    keypair_path: &std::path::Path,
    rpc_url: &str,
    broadcast: bool,
) -> Result<()> {
    let seed = parse_seed(seed)?;
    let real_dest = Pubkey::from_str(to).with_context(|| format!("bad --to pubkey: {to}"))?;
    let pool = load_pool(pool_path)?;
    let program_id = Pubkey::from_str(PROGRAM_ID).expect("valid program id");
    let signer = read_keypair_file(keypair_path)
        .map_err(|e| anyhow!("reading keypair {}: {e}", keypair_path.display()))?;

    let plan = match plan_bundle(&seed, bundle_id, real_dest, amount, &pool, k, DecoyConfig::default())
    {
        Ok(p) => p,
        Err(e @ supersonic_sdk::SdkError::PoolTooCold { .. })
        | Err(e @ supersonic_sdk::SdkError::PoolNotRepresentative { .. }) => {
            eprintln!("REFUSED: {e}");
            std::process::exit(1);
        }
        Err(e) => bail!(e),
    };

    // Observer view on stdout; the real index stays on stderr (never leaks in a pipe).
    println!("bundle {bundle_id}: K={k}, {} lamports moved", plan.total_moved());
    for (i, leg) in plan.legs.iter().enumerate() {
        println!("  leg {i}: {:>14} lamports -> {}", leg.amount, leg.dest);
    }
    eprintln!("(operator only) real leg is at index {}", plan.real_index);

    let client = RpcClient::new(rpc_url.to_string());
    let balance = client
        .get_balance(&signer.pubkey())
        .with_context(|| format!("querying balance from {rpc_url}"))?;
    if balance < plan.total_moved() {
        bail!(
            "signer {} has {} lamports but the bundle moves {} — fund it or lower K/amount",
            signer.pubkey(),
            balance,
            plan.total_moved()
        );
    }

    let blockhash = client
        .get_latest_blockhash()
        .with_context(|| format!("fetching blockhash from {rpc_url}"))?;
    let tx = build_bundle_tx(program_id, &signer, &plan, blockhash);

    if !broadcast {
        // Dry run: simulate, report, send nothing.
        let sim = client
            .simulate_transaction(&tx)
            .with_context(|| "simulating bundle")?;
        match sim.value.err {
            None => println!(
                "\nSIMULATED OK ({} compute units). Nothing was sent — re-run with --broadcast to submit.",
                sim.value.units_consumed.unwrap_or(0)
            ),
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

fn plan(
    seed: &str,
    pool_path: &std::path::Path,
    to: &str,
    amount: u64,
    k: usize,
    bundle_id: u64,
    out: Option<&std::path::Path>,
) -> Result<()> {
    let seed = parse_seed(seed)?;
    let real_dest = Pubkey::from_str(to).with_context(|| format!("bad --to pubkey: {to}"))?;
    let pool = load_pool(pool_path)?;

    // Selection is global (§pool): the decoys reproduce the real-payee distribution, so
    // the real leg is one more i.i.d. draw — no per-real profile is needed to plan.
    let plan = plan_bundle(
        &seed,
        bundle_id,
        real_dest,
        amount,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_seed_accepts_64_hex_with_or_without_prefix() {
        let bare = "aa".repeat(32);
        assert_eq!(parse_seed(&bare).unwrap(), [0xaa; 32]);
        assert_eq!(parse_seed(&format!("0x{bare}")).unwrap(), [0xaa; 32]);
    }

    #[test]
    fn parse_seed_rejects_bad_length_and_non_hex() {
        assert!(parse_seed(&"a".repeat(63)).is_err(), "wrong length must fail");
        assert!(parse_seed(&"a".repeat(65)).is_err(), "wrong length must fail");
        // Right length, but 'z' is not hex — a trust-boundary reject, not a silent 0.
        assert!(parse_seed(&"z".repeat(64)).is_err(), "non-hex must fail");
    }

    #[test]
    fn representative_target_is_deterministic() {
        assert_eq!(representative_target(17), representative_target(17));
    }

    #[test]
    fn representative_target_reproduces_the_fresh_share() {
        // DESIGN §1.3: ~36.6% of real payees are fresh. The pool must reproduce that split,
        // and it must hold for small pools too — a blocky `index % 100 < 37` would make a
        // 32-member pool all-fresh and the SDK would refuse it (PoolNotRepresentative).
        let fresh = (0u32..1000).filter(|&i| !representative_target(i).exists).count();
        let share = fresh as f64 / 1000.0;
        assert!((0.30..=0.44).contains(&share), "fresh share {share} off target ~0.37");

        let small = (0u32..32).filter(|&i| !representative_target(i).exists).count();
        assert!((6..=18).contains(&small), "small-pool fresh count {small} degenerate");
    }

    #[test]
    fn representative_history_members_have_history() {
        let hist = (0u32..100).map(representative_target).find(|p| p.exists).unwrap();
        assert!(hist.prior_sigs > 0, "a history member must carry prior signatures");
    }

    #[test]
    fn build_bundle_tx_signs_and_carries_every_leg() {
        use supersonic_sdk::PlannedLeg;
        let program_id = Pubkey::from_str(PROGRAM_ID).unwrap();
        let signer = Keypair::new();
        let plan = BundlePlan {
            legs: vec![
                PlannedLeg { dest: Pubkey::new_unique(), amount: 100, is_real: true, pool_index: None },
                PlannedLeg { dest: Pubkey::new_unique(), amount: 50, is_real: false, pool_index: Some(0) },
                PlannedLeg { dest: Pubkey::new_unique(), amount: 70, is_real: false, pool_index: Some(1) },
            ],
            real_index: 0,
            bundle_id: 1,
        };
        let tx = build_bundle_tx(program_id, &signer, &plan, Hash::default());
        // Signature verifies over the message (independent of blockhash validity).
        assert!(tx.verify().is_ok(), "the bundle tx must be validly signed");
        // The instruction references signer + system_program + one account per leg.
        assert_eq!(tx.message.instructions[0].accounts.len(), plan.legs.len() + 2);
        // Every leg's destination is present as an account key in the message.
        for leg in &plan.legs {
            assert!(tx.message.account_keys.contains(&leg.dest), "leg dest {} missing", leg.dest);
        }
    }
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
