//! # supersonic-sdk
//!
//! Client SDK for the `supersonic-tx` router. It turns a single real intent — "send `A`
//! lamports to `D`" — into an **intent-ambiguous, channel-complete bundle**: one real leg
//! plus `K-1` decoy legs, arranged so an on-chain observer cannot identify the real leg
//! with probability materially above `1/K` **in either the amount or the destination
//! channel**.
//!
//! The privacy engineering lives here, not on-chain. The program is a neutral atomic
//! executor; this SDK is what makes the noise believable, across both channels a
//! confirmed bundle exposes:
//!
//! * **Amount layer** (`amounts`) — exchangeable log-normal + roundness matching. Credited
//!   prior art from PR #1; correct and small, reimplemented with attribution (DESIGN §2.2).
//! * **Destination layer** (`pool`) — the spine, and our contribution. Decoy destinations
//!   are **not** fresh keys (which carry no history and leak at +0.6 advantage — the channel
//!   PR #1's harness holds constant). They are **warmed pool members** whose history
//!   profile distribution reproduces real payees', selected i.i.d. so the real leg is
//!   exchangeable in `(age, tx_count, recency)` too. When the pool cannot supply a matched
//!   set, [`plan_bundle`] **fails closed** rather than emit a leaking bundle.
//!
//! Everything is deterministic in `(master_seed, bundle_id)` for the amount/position draw,
//! and pool destinations are recoverable from the seed by member index — so the user, and
//! only the user, can later sweep parked funds back.
//!
//! ## What this does not close
//!
//! Matching `(age, tx_count, recency)` closes history-*existence*. It does not close
//! funding *provenance*: a warmed member funded from the signer re-links via the funding
//! graph. Breaking that link is mixing, which the non-custodial posture forbids. The
//! residual is stated open in DESIGN §4 — this SDK does not claim to close it.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use sha2::{Digest, Sha256};
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{keypair_from_seed, Keypair},
    signer::Signer,
    system_program,
};

pub mod amounts;
pub mod pool;
pub mod profile;

pub use pool::{PoolMember, ProfileModel, Selection, WarmingPool};
pub use profile::DestProfile;

/// Maximum legs per bundle — must match the on-chain `MAX_LEGS`.
pub const MAX_LEGS: usize = 16;
/// Minimum legs per bundle — must match the on-chain `MIN_LEGS`. A K=1 "bundle" carries
/// no decoy and only advertises tool use.
pub const MIN_LEGS: usize = 2;

/// Domain-separation tags for the key-derivation function.
const KDF_RNG: &[u8] = b"supersonic-tx/rng/v1";
const KDF_POOL: &[u8] = b"supersonic-tx/pool-dest/v1";
const KDF_SINK: &[u8] = b"supersonic-tx/disperse-sink/v1";

/// Tunables for the amount layer. Defaults are reasonable; the adversarial harness is
/// what justifies any change to them.
#[derive(Clone, Copy, Debug)]
pub struct DecoyConfig {
    /// Log-space standard deviation of decoy amounts around the real amount. ~0.6 keeps
    /// decoys within roughly a 0.3x–3x band of the real.
    pub sigma: f64,
    /// Probability a given decoy is snapped to the *same* decimal roundness as the real
    /// amount. `1.0` kills the roundness channel entirely.
    pub round_match_prob: f64,
    /// Plausible amount band, in lamports. Decoys are resampled to stay inside it (the
    /// support-boundary defense). The real amount is assumed inside; if not, the band is
    /// widened to include it — the real is never distorted.
    pub min_lamports: u64,
    pub max_lamports: u64,
}

impl Default for DecoyConfig {
    fn default() -> Self {
        Self {
            sigma: 0.6,
            min_lamports: 1_000_000,
            max_lamports: 100_000_000_000,
            round_match_prob: 1.0,
        }
    }
}

/// One planned leg of a bundle.
#[derive(Clone, Debug)]
pub struct PlannedLeg {
    /// Destination that will receive `amount` lamports.
    pub dest: Pubkey,
    /// Lamports moved by this leg.
    pub amount: u64,
    /// True for the single real leg; false for decoys.
    pub is_real: bool,
    /// For decoys, the **pool member index** used to recreate this destination during
    /// recovery. `None` for the real leg (its destination is user-supplied, never swept).
    pub pool_index: Option<u32>,
}

/// A fully-planned bundle: the ordered legs the transaction will contain, plus the index
/// of the real leg (which the caller keeps private and needs for recovery).
#[derive(Clone, Debug)]
pub struct BundlePlan {
    pub legs: Vec<PlannedLeg>,
    pub real_index: usize,
    pub bundle_id: u64,
}

impl BundlePlan {
    /// The per-leg amounts, in transaction order (what an observer sees).
    pub fn amounts(&self) -> Vec<u64> {
        self.legs.iter().map(|l| l.amount).collect()
    }

    /// The per-leg destinations, in transaction order.
    pub fn destinations(&self) -> Vec<Pubkey> {
        self.legs.iter().map(|l| l.dest).collect()
    }

    /// Total lamports the user pays out across all legs (principal — decoys are
    /// recoverable, so the *net* cost is fees plus whatever is left parked).
    pub fn total_moved(&self) -> u64 {
        self.legs.iter().map(|l| l.amount).sum()
    }
}

/// Derive the deterministic RNG for a bundle from the master seed and bundle id.
fn bundle_rng(master_seed: &[u8; 32], bundle_id: u64) -> ChaCha20Rng {
    let mut h = Sha256::new();
    h.update(KDF_RNG);
    h.update(master_seed);
    h.update(bundle_id.to_le_bytes());
    let seed: [u8; 32] = h.finalize().into();
    ChaCha20Rng::from_seed(seed)
}

/// Derive the keypair for pool member `index`. Deterministic in the master seed, so the
/// holder of the seed — and no one else — can recover funds parked in a warmed decoy.
///
/// Unlike PR #1's per-bundle decoy derivation, a pool member is **persistent**: it is
/// derived once, warmed over time, and reused across many bundles (that is what gives it
/// history). So its derivation is keyed on the seed and index only, not on a bundle id.
pub fn derive_pool_keypair(master_seed: &[u8; 32], index: u32) -> Keypair {
    let mut h = Sha256::new();
    h.update(KDF_POOL);
    h.update(master_seed);
    h.update(index.to_le_bytes());
    let seed: [u8; 32] = h.finalize().into();
    keypair_from_seed(&seed).expect("32-byte seed is valid")
}

/// Derive a per-decoy **dispersal sink** — a distinct user-controlled address a decoy's
/// funds are swept to, instead of straight back to the main wallet. Dispersed recovery
/// sends each decoy to its own sink in a separate transaction, so an observer sees `K-1`
/// unrelated onward transfers rather than a single star into the user's wallet (the
/// consolidation tell). Still fully recoverable from the seed.
///
/// Note: this reduces the *consolidation* tell on the recovery side; it does **not** close
/// the funding-graph residual on the warming side (DESIGN §4).
pub fn derive_sink_keypair(master_seed: &[u8; 32], index: u32) -> Keypair {
    let mut h = Sha256::new();
    h.update(KDF_SINK);
    h.update(master_seed);
    h.update(index.to_le_bytes());
    let seed: [u8; 32] = h.finalize().into();
    keypair_from_seed(&seed).expect("32-byte seed is valid")
}

/// Plan an intent-ambiguous, channel-complete bundle for a single real transfer.
///
/// * `master_seed` — the user's recovery secret (recovers pool members; hold it safe).
/// * `bundle_id` — a per-bundle nonce; combined with the seed it makes the amount and
///   position draw reproducible.
/// * `real_dest` / `real_amount` — the user's genuine intent.
/// * `real_profile` — the observed on-chain profile of `real_dest` (one
///   `getSignaturesForAddress` call). Used only to assert the pool reproduces the same
///   population; the selection never biases toward it.
/// * `pool` — the warmed destination pool. Decoys are drawn from its **eligible**
///   members.
/// * `k` — total legs including the real one, `MIN_LEGS..=MAX_LEGS`.
/// * `cfg` — amount-layer tunables.
///
/// # Errors
/// - [`SdkError::BadAnonymitySet`] / [`SdkError::ZeroRealAmount`] / [`SdkError::BadDestination`]
///   for invalid inputs.
/// - [`SdkError::PoolTooCold`] when the pool has too few mature members to match — the
///   **fail-closed** path. The SDK refuses to emit a bundle whose decoys would leak;
///   silent degradation to +0.6 advantage is worse than a refusal.
pub fn plan_bundle(
    master_seed: &[u8; 32],
    bundle_id: u64,
    real_dest: Pubkey,
    real_amount: u64,
    real_profile: DestProfile,
    pool: &WarmingPool,
    k: usize,
    cfg: DecoyConfig,
) -> Result<BundlePlan, SdkError> {
    if !(MIN_LEGS..=MAX_LEGS).contains(&k) {
        return Err(SdkError::BadAnonymitySet(k));
    }
    if real_amount == 0 {
        return Err(SdkError::ZeroRealAmount);
    }
    if real_dest == system_program::ID {
        return Err(SdkError::BadDestination);
    }

    let mut rng = bundle_rng(master_seed, bundle_id);
    let n_decoys = k - 1;

    // Destination layer FIRST, so a cold pool fails closed before we waste any work — and
    // before the amount draw, so a caller that retries a failed plan gets the same amounts
    // for a given (seed, id) once the pool warms.
    let members = match pool.select(&real_profile, k, &mut rng) {
        Selection::Matched(members) => members,
        Selection::PoolTooCold { eligible, needed } => {
            return Err(SdkError::PoolTooCold { eligible, needed })
        }
        Selection::PoolNotRepresentative {
            eligible,
            eligible_fresh_share,
            model_fresh_share,
        } => {
            return Err(SdkError::PoolNotRepresentative {
                eligible,
                eligible_fresh_share,
                model_fresh_share,
            })
        }
    };

    // Amount layer: decoys exchangeable with the real amount, roundness-matched.
    let decoy_amounts = amounts::generate_decoy_amounts(real_amount, n_decoys, &cfg, &mut rng);
    debug_assert_eq!(decoy_amounts.len(), members.len());

    // Build decoy legs: warmed, recoverable pool destinations paired with decoy amounts.
    let mut legs: Vec<PlannedLeg> = members
        .iter()
        .zip(decoy_amounts)
        .map(|(m, amount)| PlannedLeg {
            dest: derive_pool_keypair(master_seed, m.index).pubkey(),
            amount,
            is_real: false,
            pool_index: Some(m.index),
        })
        .collect();

    // Insert the real leg at a seed-determined random position.
    let real_index = rng.gen_range(0..k);
    legs.insert(
        real_index,
        PlannedLeg {
            dest: real_dest,
            amount: real_amount,
            is_real: true,
            pool_index: None,
        },
    );

    Ok(BundlePlan {
        legs,
        real_index,
        bundle_id,
    })
}

/// Build the `execute_bundle` instruction for a plan. Destinations are appended as
/// writable, non-signer accounts in leg order (the program pairs them 1:1 with the legs).
pub fn build_instruction(program_id: Pubkey, user: Pubkey, plan: &BundlePlan) -> Instruction {
    let data = execute_bundle_data(&plan.amounts());

    let mut accounts = vec![
        AccountMeta::new(user, true),
        AccountMeta::new_readonly(system_program::ID, false),
    ];
    for dest in plan.destinations() {
        accounts.push(AccountMeta::new(dest, false));
    }

    Instruction {
        program_id,
        accounts,
        data,
    }
}

/// Encode the `execute_bundle` instruction data exactly as the Anchor program expects:
/// 8-byte discriminator + Borsh `Vec<Leg>` (a `u32` length then each `Leg { amount: u64 }`
/// little-endian).
pub fn execute_bundle_data(amounts: &[u64]) -> Vec<u8> {
    let mut data = anchor_discriminator("execute_bundle").to_vec();
    data.extend_from_slice(&(amounts.len() as u32).to_le_bytes());
    for a in amounts {
        data.extend_from_slice(&a.to_le_bytes());
    }
    data
}

/// Anchor's global instruction discriminator: first 8 bytes of `sha256("global:<name>")`.
pub fn anchor_discriminator(ix_name: &str) -> [u8; 8] {
    let mut h = Sha256::new();
    h.update(format!("global:{ix_name}").as_bytes());
    let digest = h.finalize();
    let mut out = [0u8; 8];
    out.copy_from_slice(&digest[..8]);
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum SdkError {
    /// `k` outside `MIN_LEGS..=MAX_LEGS`.
    BadAnonymitySet(usize),
    ZeroRealAmount,
    BadDestination,
    /// The warmed pool cannot supply `needed` matched decoys (only `eligible` mature).
    /// The fail-closed path: no bundle is built.
    PoolTooCold { eligible: usize, needed: usize },
    /// Enough mature members, but they do not reproduce the modeled fresh/history split,
    /// so a uniform draw would leak (e.g. only fresh members matured). Also fail-closed.
    PoolNotRepresentative {
        eligible: usize,
        eligible_fresh_share: f64,
        model_fresh_share: f64,
    },
}

impl std::fmt::Display for SdkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SdkError::BadAnonymitySet(k) => {
                write!(f, "anonymity set k={k} must be in {MIN_LEGS}..={MAX_LEGS}")
            }
            SdkError::ZeroRealAmount => write!(f, "real amount must be greater than zero"),
            SdkError::BadDestination => write!(f, "invalid real destination"),
            SdkError::PoolTooCold { eligible, needed } => write!(
                f,
                "pool too cold: {eligible} mature members, need {needed} — refusing to \
                 build a bundle whose decoys would leak (warm the pool and retry)"
            ),
            SdkError::PoolNotRepresentative {
                eligible,
                eligible_fresh_share,
                model_fresh_share,
            } => write!(
                f,
                "pool not representative: {eligible} mature members are {:.0}% fresh but the \
                 model is {:.0}% — a uniform draw would leak; warm the history members too",
                eligible_fresh_share * 100.0,
                model_fresh_share * 100.0
            ),
        }
    }
}

impl std::error::Error for SdkError {}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: [u8; 32] = [7u8; 32];

    /// A fully-warmed pool of `n` members at a fixed aged target — enough to plan bundles.
    fn warm_pool(n: u32) -> WarmingPool {
        let target = DestProfile::observed(900, Some(8_000_000), Some(100));
        let model = ProfileModel::from_profiles((0..n).map(|_| target));
        let mut pool = WarmingPool::new(model);
        for i in 0..n {
            pool.members.push(PoolMember {
                index: i,
                target,
                current: target,
            });
        }
        pool
    }

    fn real_prof() -> DestProfile {
        DestProfile::observed(700, Some(6_000_000), Some(80))
    }

    #[test]
    fn plan_is_deterministic_in_seed_and_id() {
        let dest = Keypair::new().pubkey();
        let pool = warm_pool(32);
        let a = plan_bundle(&SEED, 1, dest, 1_337_000, real_prof(), &pool, 5, DecoyConfig::default()).unwrap();
        let b = plan_bundle(&SEED, 1, dest, 1_337_000, real_prof(), &pool, 5, DecoyConfig::default()).unwrap();
        assert_eq!(a.amounts(), b.amounts(), "same seed+id => same amounts");
        assert_eq!(a.destinations(), b.destinations(), "same seed+id => same dests");
        assert_eq!(a.real_index, b.real_index);
    }

    #[test]
    fn real_leg_present_exactly_once_with_right_value() {
        let dest = Keypair::new().pubkey();
        let pool = warm_pool(32);
        let plan = plan_bundle(&SEED, 9, dest, 4_200_000, real_prof(), &pool, 6, DecoyConfig::default()).unwrap();
        assert_eq!(plan.legs.len(), 6);
        let reals: Vec<_> = plan.legs.iter().filter(|l| l.is_real).collect();
        assert_eq!(reals.len(), 1, "exactly one real leg");
        assert_eq!(reals[0].dest, dest);
        assert_eq!(reals[0].amount, 4_200_000);
        assert_eq!(plan.legs[plan.real_index].dest, dest);
    }

    #[test]
    fn decoys_come_from_the_pool_and_are_recoverable_from_seed() {
        let dest = Keypair::new().pubkey();
        let pool = warm_pool(32);
        let plan = plan_bundle(&SEED, 3, dest, 2_000_000, real_prof(), &pool, 5, DecoyConfig::default()).unwrap();
        for leg in plan.legs.iter().filter(|l| !l.is_real) {
            let idx = leg.pool_index.expect("decoy carries its pool index");
            let kp = derive_pool_keypair(&SEED, idx);
            assert_eq!(kp.pubkey(), leg.dest, "decoy dest recoverable from seed by pool index");
        }
    }

    #[test]
    fn cold_pool_fails_closed_not_a_leaking_bundle() {
        let dest = Keypair::new().pubkey();
        // Model non-empty (so it could sample), but zero warmed members => cannot match.
        let pool = WarmingPool::new(ProfileModel::from_profiles([real_prof()]));
        let err = plan_bundle(&SEED, 1, dest, 1_000_000, real_prof(), &pool, 8, DecoyConfig::default())
            .unwrap_err();
        assert_eq!(err, SdkError::PoolTooCold { eligible: 0, needed: 7 });
    }

    #[test]
    fn rejects_bad_params() {
        let dest = Keypair::new().pubkey();
        let pool = warm_pool(32);
        assert!(matches!(
            plan_bundle(&SEED, 1, dest, 1_000, real_prof(), &pool, 1, DecoyConfig::default()),
            Err(SdkError::BadAnonymitySet(1))
        ));
        assert!(matches!(
            plan_bundle(&SEED, 1, dest, 0, real_prof(), &pool, 4, DecoyConfig::default()),
            Err(SdkError::ZeroRealAmount)
        ));
    }

    #[test]
    fn instruction_layout_matches_program_abi() {
        let dest = Keypair::new().pubkey();
        let pool = warm_pool(32);
        let user = Keypair::new().pubkey();
        let plan = plan_bundle(&SEED, 2, dest, 3_000_000, real_prof(), &pool, 4, DecoyConfig::default()).unwrap();
        let ix = build_instruction(Pubkey::new_unique(), user, &plan);
        // signer + system_program + K destinations.
        assert_eq!(ix.accounts.len(), 2 + 4);
        assert!(ix.accounts[0].is_signer);
        assert!(!ix.accounts[1].is_signer);
        // 8 discriminator + 4 vec-len + 4 legs * 8 bytes.
        assert_eq!(ix.data.len(), 8 + 4 + 4 * 8);
        assert_eq!(&ix.data[..8], &anchor_discriminator("execute_bundle"));
    }
}
