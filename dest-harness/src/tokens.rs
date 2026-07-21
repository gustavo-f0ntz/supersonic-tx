//! The token-holdings channel — a new open residual, measured, not yet closed.
//!
//! Every module in this crate so far measures a channel this system's `WarmingPool`
//! matches or a residual it explicitly does not close. This one is neither: it is a
//! channel the *current* warming scheme cannot close by construction, because
//! `WarmingPool` only ever ages a member via System-program transfers (§3.2) — a pool
//! member never touches the SPL Token program, so it always has **zero** token
//! accounts. A real payee is under no such constraint.
//!
//! This module quantifies how much that costs, from an out-of-tree mainnet collection
//! (`data/token_holdings.jsonl`, `getTokenAccountsByOwner` against the classic SPL Token
//! program for every destination in `dest_study.jsonl` that had prior history — a fresh
//! destination cannot yet hold a token account either, so only history payees can differ).

use serde::Deserialize;

/// One destination's classic-SPL-Token account count (Token-2022 not queried; see the
/// scoping note in `CHANNELS.md`).
#[derive(Clone, Debug, Deserialize)]
pub struct TokenRow {
    pub dest: String,
    pub token_account_count: Option<u32>,
}

pub fn load(jsonl: &str) -> anyhow::Result<Vec<TokenRow>> {
    let mut out = Vec::new();
    for (i, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(
            serde_json::from_str(line).map_err(|e| anyhow::anyhow!("token line {}: {e}", i + 1))?,
        );
    }
    Ok(out)
}

/// `(has_tokens, resolved)` over a token-holdings study: *resolved* rows are those the
/// RPC answered (all of them, in the shipped collection); *has_tokens* holds ≥1 account.
pub fn has_tokens_over_resolved(rows: &[TokenRow]) -> (usize, usize) {
    let resolved = rows
        .iter()
        .filter(|r| r.token_account_count.is_some())
        .count();
    let has_tokens = rows
        .iter()
        .filter(|r| r.token_account_count.unwrap_or(0) > 0)
        .count();
    (has_tokens, resolved)
}

/// Residual advantage of the "which leg holds an SPL token account?" attack.
///
/// A pool decoy holds exactly zero, always (the warming scheme never touches the token
/// program). The real leg holds ≥1 with probability `p_overall`; when it does, it is the
/// unique leg with tokens and the attacker reads it off directly. When it doesn't, every
/// leg looks decoy-like and the attacker is back to `1/K` — the same shape as the
/// history channel (§1.2) and the funding residual (§4): `advantage(K) = p·(1 − 1/K)`.
pub fn token_advantage(p_overall: f64, k: usize) -> f64 {
    debug_assert!(k >= 2);
    p_overall * (1.0 - 1.0 / k as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(count: Option<u32>) -> TokenRow {
        TokenRow {
            dest: "d".into(),
            token_account_count: count,
        }
    }

    #[test]
    fn counts_resolved_and_has_tokens() {
        let rows = vec![row(Some(0)), row(Some(3)), row(Some(1)), row(None)];
        let (has_tokens, resolved) = has_tokens_over_resolved(&rows);
        assert_eq!((has_tokens, resolved), (2, 3));
    }

    #[test]
    fn advantage_matches_the_history_channel_shape() {
        let p = 0.094;
        assert!((token_advantage(p, 2) - p * 0.5).abs() < 1e-9);
        assert!((token_advantage(p, 16) - p * (1.0 - 1.0 / 16.0)).abs() < 1e-9);
        assert!(token_advantage(p, 16) > token_advantage(p, 2));
    }

    #[test]
    fn no_residual_when_no_real_payee_holds_tokens() {
        assert_eq!(token_advantage(0.0, 8), 0.0);
    }
}
