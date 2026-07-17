# PROOF — the destination-history channel, measured open and closed

> Reproducible evidence for the claims in `DESIGN.md`. Every number below comes from a
> command in this repo run against real mainnet data or the real on-chain program; none
> is modelled. Re-run any of them yourself.

## 1. The empirical basis (real mainnet, n=1181)

`dest-harness/data/dest_study.jsonl` — 1181 confirmed System-program `transfer`
destinations sampled from finalized mainnet blocks. For each, `prior_sigs` counts
signatures strictly before the transfer's slot.

```
749 / 1181 real destinations had prior on-chain history  =  63.4%   (95% Wilson CI 60.6%–66.1%)
432 / 1181 were fresh (zero prior history)               =  36.6%
```

A derived decoy destination (`derive_pool_keypair` / any `derive_decoy_keypair`) has
`prior_sigs = 0` by construction — the account does not exist until the bundle creates
it. **The real and decoy populations do not overlap on this channel.**

Verify the study size and split:

```
$ wc -l dest-harness/data/dest_study.jsonl        # 1181
$ jq -s 'map(.had_history) | add' dest-harness/data/dest_study.jsonl   # 749
```

## 2. The advantage — open vs closed, one harness

```
$ cd dest-harness
$ cargo run -p supersonic-dest-harness --bin dest-advantage -- \
      --study data/dest_study.jsonl --n 8000 --seed 1
```

Output (this is the load-bearing table; the harness selects the best attack on the
train split and scores it on the held-out test split, exactly as PR #1's amount-channel
harness does, so the two are directly comparable):

| K | baseline (1/K) | **open** — best attack over fresh decoys | 95% CI | **defended** — warmed pool |
|---|---|---|---|---|
| 2  | 0.500 | **+0.317** | [+0.308, +0.325] | **−0.001** |
| 4  | 0.250 | **+0.476** | [+0.466, +0.485] | **−0.002** |
| 8  | 0.125 | **+0.550** | [+0.540, +0.560] | **+0.011** |
| 16 | 0.062 | **+0.598** | [+0.588, +0.608] | **+0.014** |

For comparison, PR #1 reports **+0.037 (K=2) → +0.012 (K=16)** on the amount channel,
measured with destinations held constant. The open destination channel is **~9× to ~50×**
larger; the defended residual (+0.014 at K=16) lands on the same order as PR #1's own
amount-channel floor (+0.012) — a small measured floor, not a suspicious exact zero.

**Two facts the table makes concrete:**

1. **The slopes are opposite.** PR #1's amount advantage *falls* with K; this channel's
   *rises* (+0.317 → +0.598), because the 1/K baseline collapses beneath a near-perfect
   classifier. "Use the largest K you can afford" is inverted here.
2. **The attack is one bit.** At K=16 all four destination classifiers score identically
   (`dest_exists`, `dest_most_txs`, `dest_oldest_age`, `dest_most_recent` all +0.601):
   with decoys at zero history, *any* history feature separates them. One
   `getSignaturesForAddress` per leg.

The `defended` column draws decoys from a `WarmingPool` whose profile distribution is
fit on the **train** split; the real legs come from the **held-out test** split, so the
closure is generalization, not decoys sampled from the same rows as the reals.

## 3. The defense fails closed — it is not a silent degradation

`supersonic-sdk::plan_bundle` refuses to emit a leaking bundle. Two refusal paths,
both exercised by unit tests (`supersonic-sdk/src/pool.rs`):

- `SdkError::PoolTooCold { eligible, needed }` — too few mature members to fill K−1 legs.
- `SdkError::PoolNotRepresentative { .. }` — eligible members exist but their fresh/history
  split departs from the model's by more than `FRESH_SHARE_TOL = 0.15`. This is the leak
  the CLI surfaced that unit tests missed: a pool matured to *fresh-only* members passes a
  naive count check but reproduces the wrong distribution and leaks. `select` now gates on
  `fresh_share`, so a pool that can't reproduce the §1.3 distribution is rejected before a
  bundle is built.

A tool that degrades to +0.6 advantage without telling you is worse than one that says no.

## 4. The bundle settles on the real program (tx-level, LiteSVM)

The SDK plan is not a paper artifact — it lands on the deployed program bytecode.
`e2e/tests/sdk_e2e.rs` loads the built `.so` by path into LiteSVM and drives an
SDK-planned bundle through it:

```
$ cargo build-sbf --manifest-path programs/supersonic-tx/Cargo.toml   # build the .so
$ cargo test -p supersonic-e2e
    test underfunded_sdk_bundle_reverts_atomically ... ok
    test sdk_planned_bundle_settles_on_program ... ok
```

- **`sdk_planned_bundle_settles_on_program`** — every one of the K legs lands at its
  SDK-derived destination with its planned amount; each decoy destination is
  independently re-derived from the master seed and asserted equal to the leg's address
  (`derive_pool_keypair(seed, index)` — decoys are recoverable, invariant I2).
- **`underfunded_sdk_bundle_reverts_atomically`** — an underfunded bundle fails and **no
  leg lands** (invariant I3, atomicity). A partial landing that exposes the real leg
  without its decoys would defeat the tool; the program forbids it.

Program ID: `D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be`
(`programs/supersonic-tx/PROGRAM_ID.txt`, matches `declare_id!`).

## 5. The whole thing, one command

```
$ cargo test --workspace
    ... 37 passed; 0 failed
```

Five crates: `programs/supersonic-tx` (router), `supersonic-sdk` (amount + destination
layers), `supersonic` (CLI), `dest-harness` (measurement), `e2e` (on-chain proof).

## 6. What this does NOT prove — read `CHANNELS.md`

The defended residual is the ceiling of a **fully-warmed** pool. It does not close the
**funding graph**: self-funded decoys re-link to the signer one hop out (§4 of
`DESIGN.md`). That residual is stated open, not measured away. See `CHANNELS.md`.
