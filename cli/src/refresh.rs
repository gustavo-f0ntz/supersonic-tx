//! Verify pool-member maturity against real on-chain state instead of a locally-asserted
//! flag. `warm --mature` fakes maturity for a demo; this checks it for real with the same
//! `getSignaturesForAddress` call an attacker would make (`supersonic_sdk::DestProfile`'s
//! own doc: "obtainable with one `getSignaturesForAddress` call per leg").

use std::str::FromStr;

use anyhow::{Context, Result};
use solana_client::rpc_client::{GetConfirmedSignaturesForAddress2Config, RpcClient};
use solana_sdk::{pubkey::Pubkey, signature::Signature};

use supersonic_sdk::DestProfile;

const PAGE_SIZE: usize = 1000;

/// Build the profile `WarmingPool` checks eligibility against from real signature slots.
/// Pure — no network — so it's unit-testable without an RPC.
pub fn profile_from_slots(current_slot: u64, slots: &[u64]) -> DestProfile {
    if slots.is_empty() {
        return DestProfile::fresh();
    }
    let oldest = slots.iter().copied().min().expect("checked non-empty");
    let newest = slots.iter().copied().max().expect("checked non-empty");
    DestProfile::observed(
        slots.len() as u32,
        Some(current_slot.saturating_sub(oldest)),
        Some(current_slot.saturating_sub(newest)),
    )
}

/// Query real signature history for `address` and build its current profile. Pages back
/// up to `max_pages` times (1000 signatures each); hitting the cap means the result is a
/// **lower bound** — an address can genuinely have more history than was paged through —
/// the same honesty this project already applies to every other RPC-derived count.
pub fn fetch_profile(
    client: &RpcClient,
    address: &Pubkey,
    max_pages: usize,
) -> Result<DestProfile> {
    let current_slot = client.get_slot().context("get_slot")?;
    let mut slots = Vec::new();
    let mut before: Option<Signature> = None;
    for _ in 0..max_pages.max(1) {
        let config = GetConfirmedSignaturesForAddress2Config {
            before,
            until: None,
            limit: Some(PAGE_SIZE),
            commitment: None,
        };
        let page = client
            .get_signatures_for_address_with_config(address, config)
            .with_context(|| format!("getSignaturesForAddress for {address}"))?;
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        slots.extend(page.iter().map(|s| s.slot));
        if page_len < PAGE_SIZE {
            break;
        }
        let last_sig = &page[page_len - 1].signature;
        before = Some(Signature::from_str(last_sig).context("parsing signature cursor")?);
    }
    Ok(profile_from_slots(current_slot, &slots))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_signatures_is_fresh() {
        assert_eq!(profile_from_slots(1_000, &[]), DestProfile::fresh());
    }

    #[test]
    fn profile_reflects_real_slot_span() {
        let p = profile_from_slots(1_000, &[900, 950, 990]);
        assert!(p.exists);
        assert_eq!(p.prior_sigs, 3);
        assert_eq!(p.age_slots, Some(100), "oldest slot 900 -> age 1000-900");
        assert_eq!(
            p.recency_slots,
            Some(10),
            "newest slot 990 -> recency 1000-990"
        );
    }

    #[test]
    fn single_signature_has_zero_age_recency_gap() {
        // Age and recency both measure from the same lone signature.
        let p = profile_from_slots(500, &[500]);
        assert_eq!(p.age_slots, Some(0));
        assert_eq!(p.recency_slots, Some(0));
    }
}
