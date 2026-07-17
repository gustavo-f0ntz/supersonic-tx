//! Measure the funding-graph residual (CHANNELS §5) from the mainnet funding study.
//!
//! ```text
//! cargo run -p supersonic-dest-harness --bin funding-residual -- \
//!     --funding data/funding_study.jsonl --study data/dest_study.jsonl
//! ```
//!
//! Reports, per K, the residual advantage of the "which leg's funder isn't the signer?"
//! attack that survives the closed history channel — with the resolution rate and a Wilson
//! CI, so the honesty of the estimate is on the table like every other number here.

use anyhow::{Context, Result};
use clap::Parser;

use supersonic_dest_harness::{
    eval::wilson_ci,
    funding::{distinct_over_resolved, funding_advantage, load},
    load_study,
};

#[derive(Parser)]
#[command(about = "Funding-graph residual advantage for supersonic-tx bundles")]
struct Args {
    /// The funding study (out-of-tree mainnet collection).
    #[arg(long, default_value = "data/funding_study.jsonl")]
    funding: String,
    /// The destination study, for the history share (only history payees can be distinct).
    #[arg(long, default_value = "data/dest_study.jsonl")]
    study: String,
    #[arg(long, value_delimiter = ',', default_value = "2,4,8,16")]
    k: Vec<usize>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let funding = load(&std::fs::read_to_string(&args.funding).with_context(|| {
        format!(
            "reading {} (run the funding collection first)",
            args.funding
        )
    })?)?;
    let study =
        load_study(&std::fs::read_to_string(&args.study).with_context(|| "reading dest study")?)?;
    anyhow::ensure!(!funding.is_empty(), "funding study is empty");

    let history_share =
        study.iter().filter(|r| r.had_history).count() as f64 / study.len().max(1) as f64;
    let (distinct, resolved) = distinct_over_resolved(&funding);
    anyhow::ensure!(
        resolved > 0,
        "no funding rows resolved — collection incomplete"
    );

    let p_distinct = distinct as f64 / resolved as f64;
    let p_overall = history_share * p_distinct;
    let (lo, hi) = wilson_ci(distinct, resolved, 1.96);

    println!("supersonic-tx — funding-graph residual (CHANNELS §5)");
    println!(
        "\nResolved {resolved}/{} funding traces ({:.0}% of history payees).",
        funding.len(),
        100.0 * resolved as f64 / funding.len() as f64
    );
    println!(
        "Third-party-funded among resolved: {distinct}/{resolved} = {:.1}% (95% CI {:.1}%–{:.1}%).",
        p_distinct * 100.0,
        lo * 100.0,
        hi * 100.0
    );
    println!(
        "History share (a fresh payee is self-funded like a decoy): {:.1}%.",
        history_share * 100.0
    );
    println!(
        "=> P(a real leg is distinct on the funding graph) = {:.1}% × {:.1}% = {:.1}%.\n",
        history_share * 100.0,
        p_distinct * 100.0,
        p_overall * 100.0
    );

    println!("  K | residual advantage | 95% CI");
    println!("----+--------------------+------------------");
    for &k in &args.k {
        anyhow::ensure!(k >= 2, "K must be >= 2");
        let adv = funding_advantage(p_overall, k);
        let adv_lo = funding_advantage(history_share * lo, k);
        let adv_hi = funding_advantage(history_share * hi, k);
        println!(" {k:>2} |        {adv:+.3}       | [{adv_lo:+.3}, {adv_hi:+.3}]");
    }
    println!(
        "\nThis is the residual the history-channel defense does NOT close: self-funded decoys\n\
         re-link to the signer, so a third-party-funded real payee stands out. Closing it needs\n\
         decoys funded by other people (the mirror-pool crowd interface, CHANNELS §5.1) — not\n\
         something any self-funded scheme, including this one, can do."
    );
    Ok(())
}
