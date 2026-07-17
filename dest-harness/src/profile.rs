//! Measurement-side destination types.
//!
//! The shared atom — [`DestProfile`], what an observer reads about a leg's destination —
//! now lives in the product crate (`supersonic_sdk::profile`), because the thing we
//! defend and the thing we measure must be one struct, and the product owns it. This
//! module re-exports it and adds the one type that is measurement-only: a sampled
//! [`Bundle`], which pairs K profiles with the (secret, for-scoring-only) real index.

pub use supersonic_sdk::profile::DestProfile;

/// One sampled bundle: `K` destination profiles in transaction order, plus the index of
/// the real leg (which the adversary is trying to recover, and which the harness knows
/// only for scoring). Purely a measurement construct — the SDK never sees a real index.
#[derive(Clone, Debug)]
pub struct Bundle {
    pub profiles: Vec<DestProfile>,
    pub real_index: usize,
}

impl Bundle {
    pub fn k(&self) -> usize {
        self.profiles.len()
    }
}
