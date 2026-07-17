//! Decoy **amount** generation — the amount layer, credited prior art.
//!
//! The exchangeable log-normal construction and roundness matching here are **not our
//! contribution**. They are @Jmkoygg's, from supersonic-tx PR #1 (`sdk/src/amounts.rs`),
//! and they are correct: the 2018 Monero gamma fix applied to payment *value*. We
//! reimplement them, with credit, because our system must own its amount layer end to
//! end — not to improve on them. Our contribution is the *destination* layer (`pool`),
//! which PR #1's harness holds constant. See DESIGN.md §2.2.
//!
//! Two ideas do the work:
//!
//! 1. **Exchangeable log-normal.** Payment sizes are roughly log-distributed. Treat the
//!    real amount as itself one draw from the bundle's `LN(mu, sigma)`: draw the real
//!    leg's own z-score `z_real ~ N(0,1)`, back out `mu = ln(real) - sigma*z_real`, then
//!    draw the decoys i.i.d. from that same `LN(mu, sigma)`. The real value is then one
//!    of K i.i.d. samples, so no amount- or position-based classifier beats `1/K` — and
//!    critically the real is neither the most central (the log-median attack) nor the
//!    most extreme value.
//! 2. **Roundness matching.** Real payments are often round (`1.0 SOL`). If decoys are
//!    jittered to precise values while the real one is round, the round leg gives itself
//!    away. So decoys' roundness levels are drawn symmetrically around the real's level,
//!    making the real's roundness a typical draw rather than a tell.
//!
//! Both are deterministic given the RNG the caller passes (seeded from the master seed),
//! which keeps the whole plan reproducible and the decoys recoverable.

use rand::Rng;

use crate::DecoyConfig;

/// Cap on how "round" we ever snap (10^9 lamports = 1 SOL); beyond this, snapping would
/// erase all variety.
const MAX_ROUND_LEVEL: u32 = 9;

/// Clamp on every leg's z-score (real and decoy alike) so a Gaussian tail can't blow the
/// user's balance. Applied uniformly, so it introduces no directional tell.
const Z_CLAMP: f64 = 3.5;

/// Generate `n` decoy amounts so that the real amount is statistically **exchangeable**
/// with them, then roundness-match them to the real. See the module docs for the
/// construction and its attribution.
pub fn generate_decoy_amounts<R: Rng>(
    real: u64,
    n: usize,
    cfg: &DecoyConfig,
    rng: &mut R,
) -> Vec<u64> {
    if n == 0 {
        return Vec::new();
    }
    let real_round = trailing_zeros_base10(real);

    // Derive the bundle center so that `real` is a genuine LN(mu, sigma) sample with its
    // own drawn z-score. The real leg is never distorted.
    let z_real = standard_normal(rng).clamp(-Z_CLAMP, Z_CLAMP);
    let mu = (real.max(1) as f64).ln() - cfg.sigma * z_real;

    // Plausible band, widened to always include the real leg. Decoys are rejection-
    // sampled into it so a decoy in a Gaussian tail can't land at an implausible size
    // and reveal itself as "not the real one" (the support-boundary attack).
    let lo = (cfg.min_lamports.min(real).max(1)) as f64;
    let hi = (cfg.max_lamports.max(real)) as f64;

    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut raw;
        let mut tries = 0;
        loop {
            let z = standard_normal(rng).clamp(-Z_CLAMP, Z_CLAMP);
            raw = (mu + cfg.sigma * z).exp();
            if (raw >= lo && raw <= hi) || tries >= 64 {
                break;
            }
            tries += 1;
        }
        let mut v = (raw.clamp(lo, hi).round() as u64).max(1);

        // round_match_prob == 0 disables roundness matching: raw exchangeable draws, no
        // grid snapping. Kept as a measurement path — removes the snap-vs-exact
        // asymmetry but re-exposes a round real.
        if cfg.round_match_prob <= 0.0 {
            out.push(v);
            continue;
        }

        // Roundness matching: decoys' roundness levels are drawn around the real's level
        // (mode = real_round), making the real's roundness a typical draw, not a tell.
        let level = if rng.gen_bool(cfg.round_match_prob) {
            real_round
        } else {
            let lo_l = real_round.saturating_sub(1);
            let hi_l = (real_round + 1).min(MAX_ROUND_LEVEL);
            rng.gen_range(lo_l..=hi_l)
        };
        v = snap_to_roundness(v, level);
        v = ((v as f64).clamp(lo, hi).round() as u64).max(1);
        out.push(v);
    }
    out
}

/// Number of trailing zeros of `x` in base 10, capped at [`MAX_ROUND_LEVEL`].
/// `1_000_000 -> 6`, `1_337_000 -> 3`, `42 -> 0`, `0 -> 0`.
pub fn trailing_zeros_base10(x: u64) -> u32 {
    if x == 0 {
        return 0;
    }
    let mut n = 0;
    let mut v = x;
    while v % 10 == 0 && n < MAX_ROUND_LEVEL {
        v /= 10;
        n += 1;
    }
    n
}

/// Round `v` to have **exactly** `level` trailing zeros in base 10 (never more), never
/// returning zero (a zero-value leg is rejected on-chain and is a trivial tell).
///
/// The "exactly, never more" part matters: a decoy that snapped to a multiple of
/// `10^(level+1)` would read as *rounder* than a real leg fixed at exactly `level`
/// zeros, letting the real stand out as the least-round leg.
pub fn snap_to_roundness(v: u64, level: u32) -> u64 {
    let m = 10u64.saturating_pow(level);
    if m <= 1 {
        return v.max(1);
    }
    let mut snapped = ((v + m / 2) / m) * m;
    if snapped == 0 {
        snapped = m;
    }
    // Break any extra trailing zero so the result has exactly `level` of them.
    if level < MAX_ROUND_LEVEL {
        let m10 = m.saturating_mul(10);
        if snapped % m10 == 0 {
            snapped += m;
        }
    }
    snapped
}

/// A standard normal sample via the Box–Muller transform.
fn standard_normal<R: Rng>(rng: &mut R) -> f64 {
    // Guard u1 away from 0 so ln() is finite.
    let u1: f64 = rng.gen_range(f64::MIN_POSITIVE..1.0);
    let u2: f64 = rng.gen_range(0.0..1.0);
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    fn rng() -> ChaCha20Rng {
        ChaCha20Rng::from_seed([13u8; 32])
    }

    #[test]
    fn trailing_zeros_are_correct() {
        assert_eq!(trailing_zeros_base10(1_000_000), 6);
        assert_eq!(trailing_zeros_base10(1_337_000), 3);
        assert_eq!(trailing_zeros_base10(42), 0);
        assert_eq!(trailing_zeros_base10(0), 0);
        assert_eq!(trailing_zeros_base10(5_000_000_000), MAX_ROUND_LEVEL); // capped
    }

    #[test]
    fn snap_never_returns_zero() {
        assert_eq!(snap_to_roundness(3, 9), 1_000_000_000);
        assert_eq!(snap_to_roundness(0, 3), 1_000);
        assert_eq!(snap_to_roundness(1_499, 3), 1_000);
        assert_eq!(snap_to_roundness(1_500, 3), 2_000);
    }

    #[test]
    fn generates_requested_count_all_positive() {
        let cfg = DecoyConfig::default();
        let v = generate_decoy_amounts(1_337_000, 7, &cfg, &mut rng());
        assert_eq!(v.len(), 7);
        assert!(v.iter().all(|&a| a > 0));
    }

    #[test]
    fn real_is_exchangeable_not_systematically_extreme() {
        // Exchangeability means the real leg is the global min OR max at ~2/K — the same
        // rate as any single leg — not systematically central nor extreme. K=8 => 0.25.
        let cfg = DecoyConfig::default();
        let real = 1_337_000u64;
        let k = 8usize;
        let trials = 4000;
        let mut extreme = 0;
        for i in 0..trials {
            let mut r = ChaCha20Rng::seed_from_u64(i);
            let decoys = generate_decoy_amounts(real, k - 1, &cfg, &mut r);
            let min = *decoys.iter().min().unwrap();
            let max = *decoys.iter().max().unwrap();
            if real <= min || real >= max {
                extreme += 1;
            }
        }
        let rate = extreme as f64 / trials as f64;
        let expected = 2.0 / k as f64; // 0.25
        assert!(
            (rate - expected).abs() < 0.06,
            "real should be the extreme at ~{expected:.2} (exchangeable), got {rate:.3}"
        );
    }

    #[test]
    fn round_real_is_hidden_among_round_decoys() {
        let cfg = DecoyConfig::default();
        let real = 1_000_000_000u64; // 1 SOL, very round
        let decoys = generate_decoy_amounts(real, 10, &cfg, &mut rng());
        let round_like = decoys
            .iter()
            .filter(|&&a| trailing_zeros_base10(a) >= 6)
            .count();
        assert!(
            round_like >= 3,
            "a round real needs round company, got {round_like}/10 round-ish decoys"
        );
    }
}
