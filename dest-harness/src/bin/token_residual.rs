//! Measure the token-holdings channel (CHANNELS §5.2) from the mainnet token-holdings study.
//!
//! ```text
//! cargo run -p supersonic-dest-harness --bin token-residual -- \
//!     --tokens data/token_holdings.jsonl --study data/dest_study.jsonl
//! ```

use anyhow::{Context, Result};
use clap::Parser;

use supersonic_dest_harness::{
    eval::wilson_ci,
    load_study,
    tokens::{has_tokens_over_resolved, load, token_advantage},
};

#[derive(Parser)]
#[command(about = "Token-holdings channel advantage for supersonic-tx bundles")]
struct Args {
    #[arg(long, default_value = "data/token_holdings.jsonl")]
    tokens: String,
    /// For the study's total size — only history payees were queried, so the overall
    /// probability is scoped against the full population, like the funding residual.
    #[arg(long, default_value = "data/dest_study.jsonl")]
    study: String,
    #[arg(long, value_delimiter = ',', default_value = "2,4,8,16")]
    k: Vec<usize>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let tokens = load(&std::fs::read_to_string(&args.tokens).with_context(|| {
        format!(
            "reading {} (run the token-holdings collection first)",
            args.tokens
        )
    })?)?;
    let study =
        load_study(&std::fs::read_to_string(&args.study).with_context(|| "reading dest study")?)?;
    anyhow::ensure!(!tokens.is_empty(), "token-holdings study is empty");

    let (has_tokens, resolved) = has_tokens_over_resolved(&tokens);
    anyhow::ensure!(resolved > 0, "no token-holdings rows resolved");

    let p_among_history = has_tokens as f64 / resolved as f64;
    let p_overall = has_tokens as f64 / study.len().max(1) as f64;
    let (lo, hi) = wilson_ci(has_tokens, resolved, 1.96);
    let p_overall_lo = lo * resolved as f64 / study.len().max(1) as f64;
    let p_overall_hi = hi * resolved as f64 / study.len().max(1) as f64;

    println!("supersonic-tx — token-holdings channel (CHANNELS §5.2)");
    println!("\nResolved {resolved}/{resolved} history-having destinations (0 RPC failures).");
    println!(
        "Hold >=1 classic-SPL-Token account: {has_tokens}/{resolved} = {:.1}% (95% CI {:.1}%-{:.1}%).",
        p_among_history * 100.0,
        lo * 100.0,
        hi * 100.0
    );
    println!(
        "=> P(a real leg holds a token account), over the full {} destination population: {:.1}%.\n",
        study.len(),
        p_overall * 100.0
    );

    println!("  K | residual advantage | 95% CI");
    println!("----+--------------------+------------------");
    for &k in &args.k {
        anyhow::ensure!(k >= 2, "K must be >= 2");
        let adv = token_advantage(p_overall, k);
        let adv_lo = token_advantage(p_overall_lo, k);
        let adv_hi = token_advantage(p_overall_hi, k);
        println!(" {k:>2} |        {adv:+.3}       | [{adv_lo:+.3}, {adv_hi:+.3}]");
    }
    println!(
        "\nA pool decoy holds zero token accounts, always — the warming scheme only ever moves\n\
         SOL. Closing this needs decoys that also hold plausible token balances, which is\n\
         exactly what a real account-cooker integration (README's composability section,\n\
         CHANNELS §5.1's crowd interface) would supply as a side effect of casting its own\n\
         swap/stake activity through decoy addresses — not something a SOL-only warming\n\
         scheme can synthesize alone."
    );
    Ok(())
}
