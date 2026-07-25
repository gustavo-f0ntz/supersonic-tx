# AUDIT — self-run against `solanabr/auditor-skill`

An independent security-checklist pass against this exact code, using the bounty org's
own published audit framework, [`solanabr/auditor-skill`](https://github.com/solanabr/auditor-skill)
(20 checklists, 1,346 items, 131 known attack vectors). Run in non-plugin mode: the
skill's checklists and known-vectors were read directly and applied by hand against this
repo's actual source, file by file — not a mechanical scan.

## Scope

This is a k-leg atomic transfer router plus off-chain client tooling, not a DeFi
protocol — no AMM, lending, perps, oracle, stablecoin, governance, bridge, NFT-marketplace,
or launchpad surface exists, so the checklist items scoped to those domains are N/A and
were not force-fit. In scope: the on-chain program's access-control/CPI/arithmetic
checklist items, the off-chain-Rust checklist (SDK/CLI hold key material), and the
always-applied domains (secrets handling, supply chain, opsec).

## Findings

### 1. HIGH — `bundle_id` defaulted to a constant, reusing the decoy set across bundles

`bundle_rng(master_seed, bundle_id)` derives the RNG that selects decoy pool members from
`(seed, bundle_id)` alone, before the real destination or amount are ever touched
(`supersonic-sdk/src/lib.rs`). The CLI's `plan`/`send` defaulted `--bundle-id` to the
constant `1`. Consequence: a user who ran either command without the flag — the natural
usage, since nothing marked it required — got the **identical** K−1 decoy destinations on
every real transfer. An observer watching that signer's bundle history would see the same
addresses recur with only one changing each time, identifying the real leg as "whichever
address is different this time" — collapsing the anonymity set to 1, independent of K.
This is the exact failure the tool exists to prevent, triggered by the friendliest
possible CLI usage.

**Fixed:** `--bundle-id` is now `Option<u64>` with no default. Omitted, a fresh random
`u64` is generated per invocation (`resolve_bundle_id`); passed explicitly, it reproduces
a specific past plan (the retry/recovery path `plan_is_deterministic` exists for).
Regression test: `resolve_bundle_id_does_not_default_to_a_constant` draws 200 omitted
calls and asserts they aren't all equal — fails immediately if a fixed default is ever
reintroduced.

### 2. LOW/MEDIUM — seed and KDF intermediate buffers were never zeroized

The master seed, and the SHA-256 digests `bundle_rng`/`derive_pool_keypair`/
`derive_sink_keypair` derive from it, sat in plain `[u8; 32]` locals with no `Drop`-time
wipe — recoverable from a core dump or swapped memory. Rated Low/Medium rather than the
checklist's baseline severity: the CLI is a short-lived process per invocation, not a
long-running signer daemon, so the exposure window is small. Still real, given the tool's
entire value proposition is secrecy of the seed.

**Fixed:** `zeroize::Zeroizing` now wraps the parsed seed in every CLI command
(`warm`/`plan`/`send`/`recover`) and the KDF digest buffers inside the three derivation
functions in `supersonic-sdk`. Residual, stated not hidden: `solana_sdk::Keypair`'s own
byte storage is outside this project's control and is not zeroizing-aware — this closes
the seed/digest side, not the final signing-key bytes.

## Checked and clean

- **On-chain program:** single instruction, no PDAs, no persistent state, no admin path.
  Signer check present, `system_program::transfer` CPI built with `CpiContext::new`
  (no `invoke_signed` misuse — the mover is the real signer, not a program authority).
  Every `require!` (leg-count bounds, account-count match, non-zero amount, no self-send)
  fails closed. Zero `unsafe` anywhere in the workspace.
- **Arithmetic:** `overflow-checks = true` set explicitly in the release profile — the
  SDK's summed totals panic on overflow rather than wrap. Amount generation clamps into
  `[lo, hi]` before the `f64 -> u64` cast, and Rust's saturating cast semantics mean an
  out-of-range result saturates rather than producing UB.
- **Local storage:** `pool.json` holds only public pool metadata; the observer-view file
  `plan --out` writes contains no real-leg index (it's routed to stderr only, by design,
  so a piped stdout capture never contains it). No plaintext-sensitive-data-at-rest issue.
- **Secrets in git:** `program-keypair.json` exists on disk but is `.gitignore`d and was
  never committed — verified against the full history (`git log --all -p`), not just the
  working tree.
- **CLI trust boundaries:** the previously-fixed `parse_seed` non-ASCII panic has 3
  regression tests. Every other argument (`--to`, `--amount`, `--k`, `--keypair`) goes
  through `clap`'s typed parsers or an explicit `Result`-returning conversion — none panic
  on malformed input.
- **Fail-closed paths:** `WarmingPool::select`'s `PoolTooCold`/`PoolNotRepresentative`
  are wired to a non-zero exit in both `plan` and `send`, with no flag to bypass and build
  a bundle from a cold or unrepresentative pool anyway.
- **Supply chain:** `Cargo.lock` is committed, 663 packages pinned; `anchor-lang 0.31.1`/
  `solana-sdk 2.2` are current, not stale. `cargo audit` (`RustSec` advisory DB, 1,169
  advisories) found 5 real vulnerabilities and 11 unmaintained/unsound warnings — traced
  each with `cargo tree -i` before writing this: all 5 (`curve25519-dalek` 3.2.0,
  `ed25519-dalek` 1.0.1, `rustls-webpki` 0.101.7 ×3) come transitively through
  `solana-keypair`/`solana-client`/`solana-sdk`'s own dependency choices, not anything
  this project's own `Cargo.toml` selects — not fixable without Solana Labs bumping their
  own internal pins. Same finding class the PR #1 competitor documented for their stack.

## What this pass did not re-litigate

The funding-graph residual (86.5% third-party-funded among durable payees, +0.27…+0.51)
and the token-holdings residual (+0.05…+0.09) are measured and honestly left open in
`DESIGN.md §4`/`CHANNELS.md §5` — deliberate scope limits, not bugs.
