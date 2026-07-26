//! Live devnet proof for the Pinocchio router (`bench/pinocchio-router`) — the same
//! exercise `PROOF.md §4.1` did for the Anchor program: deploy for real, send a real
//! signed K-leg bundle, confirm it settles on chain. Jmkoygg's own Anchor-vs-Pinocchio
//! benchmark (the prior art `BENCHMARK.md` credits) explicitly leaves *its* Pinocchio
//! program undeployed ("not deployed anywhere, offered as an option") — this test is
//! the one thing that benchmark class stops short of, run for real.
//!
//! `#[ignore]`d: needs network access and a funded devnet keypair, neither available in
//! CI. Run manually:
//!   `cargo test -p supersonic-dest-harness --test pinocchio_devnet -- --ignored --nocapture`
//!
//! Deploy (once, already done for the signature recorded in `BENCHMARK.md`):
//!   `solana program deploy bench/pinocchio-router/target/deploy/supersonic_tx_pinocchio.so \
//!      --url devnet --program-id bench/pinocchio-router/target/deploy/supersonic_tx_pinocchio-keypair.json`

use std::str::FromStr;

use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{read_keypair_file, Keypair, Signer},
    system_program,
    transaction::Transaction,
};

/// Deployed live on devnet — see `BENCHMARK.md` for the deploy signature.
const PROGRAM_ID: &str = "FzN88QUEZCbj2D2H7xYe9XNFrPyUHreqkPLAG7y5yaUK";
const DEVNET_RPC: &str = "https://api.devnet.solana.com";

fn encode(amounts: &[u64]) -> Vec<u8> {
    let mut data = vec![amounts.len() as u8];
    for a in amounts {
        data.extend_from_slice(&a.to_le_bytes());
    }
    data
}

#[test]
#[ignore]
fn k4_bundle_settles_on_live_devnet() {
    let program_id = Pubkey::from_str(PROGRAM_ID).unwrap();
    let payer =
        read_keypair_file(std::env::var("SOLANA_KEYPAIR").unwrap_or_else(|_| {
            format!("{}/.config/solana/id.json", std::env::var("HOME").unwrap())
        }))
        .expect("readable devnet-funded keypair (default: ~/.config/solana/id.json)");

    // Genuinely random (CSPRNG-backed) fresh addresses — not `Pubkey::new_unique()`,
    // whose deterministic per-process counter starting at a fixed low value produces
    // the same pubkeys on every run, some of which turn out to already carry a real
    // balance on the shared public devnet (confirmed: hit exactly this on a first
    // attempt, a real methodology bug this fixes, not a program bug — the transfer
    // itself settled correctly, only the "starts at zero" assumption was wrong).
    let dests: Vec<Pubkey> = (0..4).map(|_| Keypair::new().pubkey()).collect();
    let amounts = [1_000_000u64, 1_337_000, 2_500_000, 900_000];

    let mut accounts = vec![AccountMeta::new(payer.pubkey(), true)];
    for d in &dests {
        accounts.push(AccountMeta::new(*d, false));
    }
    accounts.push(AccountMeta::new_readonly(system_program::id(), false));

    let ix = Instruction {
        program_id,
        accounts,
        data: encode(&amounts),
    };

    let client = RpcClient::new(DEVNET_RPC.to_string());
    let bh = client.get_latest_blockhash().expect("fetch blockhash");
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh);

    let sig = client
        .send_and_confirm_transaction(&tx)
        .expect("bundle must settle on devnet");
    println!("K=4 bundle settled: https://explorer.solana.com/tx/{sig}?cluster=devnet");

    for (dest, amt) in dests.iter().zip(amounts.iter()) {
        let balance = client.get_balance(dest).expect("fetch dest balance");
        assert_eq!(
            balance, *amt,
            "each destination funded exactly its leg amount"
        );
    }
}
