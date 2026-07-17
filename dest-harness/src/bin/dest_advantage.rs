//! Measure the destination-channel advantage against PR #1's bundle construction.
//!
//! ```text
//! cargo run -p supersonic-dest-harness --release -- --study data/dest_study.jsonl --n 8000 --seed 1
//! ```
//!
//! Reports, per anonymity-set size K, the advantage of the best destination-channel
//! attack over the `1/K` baseline — selected on train, scored on test, exactly as
//! PR #1's amount-channel harness does, so the two tables are directly comparable.

use anyhow::{Context, Result};
use clap::Parser;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use serde::Serialize;

use supersonic_dest_harness::{
    classifiers::DestClassifier,
    eval::{best_attack, score, wilson_ci},
    load_study, pool::ProfileModel, sample_bundles, sample_bundles_defended, split,
};

#[derive(Parser, Debug)]
#[command(about = "Destination-history channel advantage for supersonic-tx bundles")]
struct Args {
    /// JSONL study of real mainnet transfer destinations.
    #[arg(long, default_value = "data/dest_study.jsonl")]
    study: String,
    /// Bundles per split, per K.
    #[arg(long, default_value_t = 8000)]
    n: usize,
    /// Anonymity-set sizes to report.
    #[arg(long, value_delimiter = ',', default_value = "2,4,8,16")]
    k: Vec<usize>,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Emit machine-readable JSON for PROOF.md.
    #[arg(long)]
    json: bool,
}

#[derive(Serialize)]
struct KRow {
    k: usize,
    baseline: f64,
    best_attack: String,
    accuracy: f64,
    advantage: f64,
    ci95_low: f64,
    ci95_high: f64,
    n_test: usize,
    /// Advantage of the best attack once decoys come from the warmed pool instead of
    /// fresh keys — the channel closed. Near 0 means the defense holds.
    defended_advantage: f64,
    defended_best_attack: String,
}

#[derive(Serialize)]
struct Report {
    study_rows: usize,
    real_payees_with_history: usize,
    share_with_history: f64,
    share_ci95: (f64, f64),
    seed: u64,
    rows: Vec<KRow>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let raw = std::fs::read_to_string(&args.study)
        .with_context(|| format!("reading study {}", args.study))?;
    let study = load_study(&raw)?;
    anyhow::ensure!(!study.is_empty(), "study is empty");

    let with_history = study.iter().filter(|r| r.had_history).count();
    let share = with_history as f64 / study.len() as f64;
    let share_ci = wilson_ci(with_history, study.len(), 1.96);

    // Train/test split of the study: the pool model is fit on train, the real legs of
    // every bundle (open and defended alike) are drawn from test, so a measured
    // closure is generalization — not decoys drawn from the same rows as the reals.
    let mut split_rng = ChaCha20Rng::seed_from_u64(args.seed ^ 0x5D17);
    let (train, test) = split(&study, &mut split_rng);
    let model = ProfileModel::fit(&train);

    let mut rows = Vec::new();
    for &k in &args.k {
        anyhow::ensure!(k >= 2, "K must be >= 2, got {k}");
        let mut rng = ChaCha20Rng::seed_from_u64(args.seed ^ (k as u64) << 32);

        // Open channel — PR #1's construction: fresh decoys, real legs from test.
        let open_tr = sample_bundles(&test, k, args.n, &mut rng);
        let open_te = sample_bundles(&test, k, args.n, &mut rng);
        let open = best_attack(&open_tr, &open_te).context("no classifiers")?;
        let hits = (open.test.accuracy * open_te.len() as f64).round() as usize;
        let (lo, hi) = wilson_ci(hits, open_te.len(), 1.96);

        // Defended channel — decoys from the warmed pool.
        let def_tr = sample_bundles_defended(&test, &model, k, args.n, &mut rng);
        let def_te = sample_bundles_defended(&test, &model, k, args.n, &mut rng);
        let defended = best_attack(&def_tr, &def_te).context("no classifiers")?;

        rows.push(KRow {
            k,
            baseline: 1.0 / k as f64,
            best_attack: open.classifier.name().to_string(),
            accuracy: open.test.accuracy,
            advantage: open.test.advantage,
            ci95_low: lo - 1.0 / k as f64,
            ci95_high: hi - 1.0 / k as f64,
            n_test: open_te.len(),
            defended_advantage: defended.test.advantage,
            defended_best_attack: defended.classifier.name().to_string(),
        });
    }

    let report = Report {
        study_rows: study.len(),
        real_payees_with_history: with_history,
        share_with_history: share,
        share_ci95: share_ci,
        seed: args.seed,
        rows,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!("supersonic-tx — destination-history channel");
    println!(
        "\nEmpirical basis: {}/{} real mainnet transfer destinations had prior history \
         = {:.1}% (95% CI {:.1}%–{:.1}%)",
        with_history,
        study.len(),
        share * 100.0,
        share_ci.0 * 100.0,
        share_ci.1 * 100.0
    );
    println!("Decoy destinations (derive_decoy_keypair): 0% have prior history, by construction.\n");

    println!("  K | baseline | open: best attack | advantage | 95% CI          || defended | attack");
    println!("----+----------+-------------------+-----------+-----------------++----------+-----------------");
    for r in &report.rows {
        println!(
            " {:>2} |    {:.3} | {:>17} |   {:+.3}  | [{:+.3},{:+.3}] || {:+.3}   | {}",
            r.k, r.baseline, r.best_attack, r.advantage, r.ci95_low, r.ci95_high,
            r.defended_advantage, r.defended_best_attack
        );
    }
    println!(
        "\n'defended' = decoys drawn from a warmed pool whose profile distribution reproduces\n\
         the real-payee population (train split); real legs from the held-out test split."
    );

    println!("\nPer-classifier advantage on test (K = {}):", report.rows.last().map(|r| r.k).unwrap_or(0));
    if let Some(&k) = args.k.last() {
        let mut rng = ChaCha20Rng::seed_from_u64(args.seed ^ (k as u64) << 32 ^ 0xBEEF);
        let test = sample_bundles(&study, k, args.n, &mut rng);
        for &c in DestClassifier::all() {
            let s = score(c, &test);
            println!("  {:>18}  {:+.3}", c.name(), s.advantage);
        }
    }

    println!(
        "\nFor comparison, PR #1 reports +0.037 (K=2) to +0.012 (K=16) on the amount channel,\n\
         measured with the destination channel held constant (fresh keys on every leg)."
    );
    Ok(())
}
