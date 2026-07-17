//! The funding-graph residual (CHANNELS §5) — measured, not modelled.
//!
//! Matching `(age, tx_count, recency)` closes history-*existence*. It does not close
//! **provenance**: a warmed decoy funded by the signer re-links via the funding graph. The
//! attack is one hop out: "the real leg is the destination whose first funder is NOT the
//! bundle signer." Every self-funded decoy has `funder == signer`; a real payee funded by a
//! third party does not, so it stays distinguishable even after the history channel closes.
//!
//! This module quantifies that residual from an out-of-tree mainnet collection
//! (`data/funding_study.jsonl`, produced over RPC the same way `dest_study.jsonl` is): for
//! each real payee that *had* history, the first funder of its destination, and whether it
//! is the paying signer. Fresh payees are funded at bundle time like decoys, so they are
//! self-funded-equivalent and safe — only history payees can be distinct.

use serde::Deserialize;

/// One resolved (or unresolved) funding trace for a real history payee.
#[derive(Clone, Debug, Deserialize)]
pub struct FundingRow {
    pub dest: String,
    pub signer: String,
    /// The first funder of `dest`, or `None` when it could not be resolved (see `status`).
    pub funder: Option<String>,
    /// `ok` / `capped` (resolved) or `tx_pruned` / `no_credit` / `no_sigs` (unresolved).
    pub status: String,
    /// `Some(true)` when the first funder is the paying signer (self-funded, decoy-like);
    /// `Some(false)` when a third party funded it (distinguishable); `None` if unresolved.
    pub funded_by_signer: Option<bool>,
}

pub fn load(jsonl: &str) -> anyhow::Result<Vec<FundingRow>> {
    let mut out = Vec::new();
    for (i, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(
            serde_json::from_str(line)
                .map_err(|e| anyhow::anyhow!("funding line {}: {e}", i + 1))?,
        );
    }
    Ok(out)
}

/// Counts over a funding study: `(distinct, resolved)` where *resolved* is the number of
/// rows whose funder was found and *distinct* those whose funder is not the signer.
pub fn distinct_over_resolved(rows: &[FundingRow]) -> (usize, usize) {
    let resolved = rows.iter().filter(|r| r.funded_by_signer.is_some()).count();
    let distinct = rows
        .iter()
        .filter(|r| r.funded_by_signer == Some(false))
        .count();
    (distinct, resolved)
}

/// Residual advantage of the funding-graph attack for a K-bundle.
///
/// A decoy is always self-funded; the real leg is *distinct* (funder ≠ signer) with
/// probability `p_distinct_overall`. When it is, the attacker reads it off directly; when
/// it is not, every leg looks self-funded and the attacker is back to `1/K`. So
/// `advantage(K) = p_distinct_overall · (1 − 1/K)` — the same shape as the history channel
/// (§1.2), with `p_distinct_overall` playing the role "has history" played there.
///
/// `p_distinct_overall = history_share · (distinct / resolved)`: only history payees can be
/// distinct, and among them the measured fraction funded by a third party.
pub fn funding_advantage(p_distinct_overall: f64, k: usize) -> f64 {
    debug_assert!(k >= 2);
    p_distinct_overall * (1.0 - 1.0 / k as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(funded_by_signer: Option<bool>, status: &str) -> FundingRow {
        FundingRow {
            dest: "d".into(),
            signer: "s".into(),
            funder: funded_by_signer.map(|_| "f".into()),
            status: status.into(),
            funded_by_signer,
        }
    }

    #[test]
    fn counts_only_resolved_rows() {
        let rows = vec![
            row(Some(false), "ok"),     // distinct
            row(Some(false), "capped"), // distinct
            row(Some(true), "ok"),      // self-funded
            row(None, "tx_pruned"),     // unresolved — excluded from both
            row(None, "no_credit"),     // unresolved
        ];
        let (distinct, resolved) = distinct_over_resolved(&rows);
        assert_eq!((distinct, resolved), (2, 3));
    }

    #[test]
    fn advantage_matches_the_history_channel_shape() {
        // If every history payee is third-party-funded (p_distinct among history = 1) and
        // 63.4% of payees have history, p_overall = 0.634, and the residual rises with K
        // toward that ceiling exactly like the open history channel.
        let p_overall = 0.634;
        assert!((funding_advantage(p_overall, 2) - 0.634 * 0.5).abs() < 1e-9);
        assert!((funding_advantage(p_overall, 16) - 0.634 * (1.0 - 1.0 / 16.0)).abs() < 1e-9);
        // Monotone increasing in K, bounded by p_overall.
        assert!(funding_advantage(p_overall, 16) > funding_advantage(p_overall, 2));
        assert!(funding_advantage(p_overall, 1_000_000) < p_overall);
    }

    #[test]
    fn no_residual_when_every_payee_is_self_funded() {
        // p_distinct = 0 (all decoy-like): the channel carries nothing.
        assert_eq!(funding_advantage(0.0, 8), 0.0);
    }
}
