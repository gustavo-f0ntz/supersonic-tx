//! The warming pool — moved to the product crate.
//!
//! The pool *is* the destination-layer defense, so it belongs to the SDK (the product),
//! not to this harness (its measurement). The harness re-exports it here so the adversary
//! measures exactly the construction the SDK ships — same code, no drift. See
//! `supersonic_sdk::pool` for the implementation and its tests.

pub use supersonic_sdk::pool::{PoolMember, ProfileModel, Selection, WarmingPool};
