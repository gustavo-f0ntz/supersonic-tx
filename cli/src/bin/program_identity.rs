//! Measures a channel this project had not named: using a single, fixed, known
//! `program_id` means anyone can enumerate every signer who has ever cast a bundle
//! through it — "this wallet uses supersonic-tx" — with zero access to which leg of any
//! bundle was real. That's a different adversary question than the rest of this project
//! answers (K-anonymity *within* a confirmed bundle); this measures it directly against
//! the real deployed program, the same way every other number in this project is
//! produced: from real chain data, not asserted.
//!
//! `cargo run -p supersonic-cli --bin program-identity -- --rpc <url>`

use std::collections::BTreeSet;
use std::str::FromStr;

use anyhow::{Context, Result};
use clap::Parser;
use solana_client::rpc_client::{GetConfirmedSignaturesForAddress2Config, RpcClient};
use solana_sdk::{pubkey::Pubkey, signature::Signature};
use solana_transaction_status_client_types::UiTransactionEncoding;

const PAGE_SIZE: usize = 1000;

#[derive(Parser)]
struct Args {
    /// RPC endpoint (the cluster the program is deployed on).
    #[arg(long)]
    rpc: String,
    /// The deployed program id. Defaults to this project's devnet deployment.
    #[arg(long, default_value = "D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be")]
    program: String,
    /// Signature pages (1000 each) to walk back. A lower bound if the cap is hit.
    #[arg(long, default_value_t = 5)]
    max_pages: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let program = Pubkey::from_str(&args.program).context("bad --program pubkey")?;
    let client = RpcClient::new(args.rpc);

    let mut signatures = Vec::new();
    let mut before: Option<Signature> = None;
    for _ in 0..args.max_pages.max(1) {
        let config = GetConfirmedSignaturesForAddress2Config {
            before,
            until: None,
            limit: Some(PAGE_SIZE),
            commitment: None,
        };
        let page = client
            .get_signatures_for_address_with_config(&program, config)
            .context("getSignaturesForAddress on the program itself")?;
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        let last = page[page_len - 1].signature.clone();
        signatures.extend(page.into_iter().map(|s| s.signature));
        if page_len < PAGE_SIZE {
            break;
        }
        before = Some(Signature::from_str(&last)?);
    }

    let mut signers = BTreeSet::new();
    for sig_str in &signatures {
        let sig = Signature::from_str(sig_str).context("parsing a returned signature")?;
        let tx = client
            .get_transaction(&sig, UiTransactionEncoding::Base64)
            .with_context(|| format!("getTransaction {sig}"))?;
        if let Some(fee_payer) = tx
            .transaction
            .transaction
            .decode()
            .and_then(|vt| vt.message.static_account_keys().first().copied())
        {
            signers.insert(fee_payer);
        }
    }

    println!(
        "program {program}: {} confirmed signature(s), {} distinct signer(s) — \
         enumerable with zero access to any bundle's real leg",
        signatures.len(),
        signers.len()
    );
    for s in &signers {
        println!("  {s}");
    }
    if signatures.len() >= args.max_pages.max(1) * PAGE_SIZE {
        println!("(hit --max-pages cap: these are lower bounds, not exhaustive)");
    }
    Ok(())
}
