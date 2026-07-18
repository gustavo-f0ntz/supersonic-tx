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
| 8  | 0.125 | **+0.550** | [+0.540, +0.560] | **+0.010** |
| 16 | 0.062 | **+0.598** | [+0.588, +0.608] | **+0.007** |

For comparison, PR #1 reports **+0.037 (K=2) → +0.012 (K=16)** on the amount channel,
measured with destinations held constant. The open destination channel is **~9× to ~50×**
larger; the defended residual (a small floor, +0.007 at K=16) lands on the same order as PR
#1's own amount-channel floor (+0.012) — a small measured floor, not a suspicious exact zero.

**Two facts the table makes concrete:**

1. **The slopes are opposite.** PR #1's amount advantage *falls* with K; this channel's
   *rises* (+0.317 → +0.598), because the 1/K baseline collapses beneath a near-perfect
   classifier. "Use the largest K you can afford" is inverted here.
2. **The attack is one bit.** At K=16 all four destination classifiers score identically
   (`dest_exists`, `dest_most_txs`, `dest_oldest_age`, `dest_most_recent` all +0.601):
   with decoys at zero history, *any* history feature separates them. One
   `getSignaturesForAddress` per leg.

The `defended` column draws its decoys through the **real `WarmingPool::select` path** —
a warmed pool of matured members reproducing the train distribution, with the same maturity
gate and fresh-share check a user's SDK runs — and the real legs come from the **held-out
test** split, so the closure is generalization *and* is produced by the deployed selection
code, not a model standing in for it. (The unit test `select_path_closes_the_channel`
reproduces the same closure on an independent synthetic pool, so the property is pinned in
CI as well as in the published run.)

> **Population note.** The study samples *all* System-program transfers; §6 finds that ~73%
> of those destinations are transient token accounts (swap plumbing), not durable payees. A
> fresh swap-ATA does not leak on the history channel (it has no history, like a decoy), so
> the +0.60 headline is measured over the *blended* transfer population and is **conservative
> for durable P2P payees** — the case this tool targets — who almost all have history.
> Concretely: the advantage tracks the history share as `≈ P(history)·(1−1/K)` (the
> `advantage_tracks_the_share_of_real_payees_with_history` test pins this), so at P≈63% it is
> +0.60, and for durable payees at P→~100% it is **+0.94 at K=16**, not +0.60. The number
> shown understates the leak for the real use case.

### 2.1 It also holds against a *learned* adversary, not just the best single bit

The table above picks the best single classifier. The obvious next question — "does a
combined attack over all features do better?" — is measured too: a conditional-logit
adversary (`dest-harness/src/learned.rs`) fits weights over `(exists, prior_sigs, age,
recency)` jointly on the train split and predicts on test.

| K | learned, open | learned, defended | (best single, defended) |
|---|---|---|---|
| 2  | +0.317 | **+0.025** | −0.001 |
| 4  | +0.476 | **+0.026** | −0.002 |
| 8  | +0.550 | **+0.012** | +0.010 |
| 16 | +0.598 | **+0.009** | +0.007 |

Two honest readings. (1) On the **open** channel the union attack recovers exactly the
single-bit leak — with decoys at zero history there is nothing to combine. (2) On the
**defended** pool it extracts a slightly *larger* residual than any single classifier at
low K (+0.025 vs −0.001 at K=2) — the small distributional slack the pool's
`FRESH_SHARE_TOL` permits is the most a combined attacker can turn into signal. That
residual is real, stated, and still well over an order of magnitude below the open leak and
on the same order as PR #1's amount-channel floor (+0.012). The defense is not beating one
hand-picked classifier; it holds against the union.

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

## 4.1 It also settles on live devnet — a real bundle, on chain

Not just LiteSVM: the program is **deployed to devnet** under the same id, and the CLI
planned and broadcast a real K=4 bundle through it end-to-end.

- **Program (devnet):** [`D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be`](https://explorer.solana.com/address/D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be?cluster=devnet)
- **Live bundle tx (status Ok):** [`5zd9D1V51Grjjm9tB38jimgdAHwAb3n5s4gqgwYUQgx5FMoygJpKKkEhVciZCNGS56NVxZJKNDsq65ADw8GtojjR`](https://explorer.solana.com/tx/5zd9D1V51Grjjm9tB38jimgdAHwAb3n5s4gqgwYUQgx5FMoygJpKKkEhVciZCNGS56NVxZJKNDsq65ADw8GtojjR?cluster=devnet)

```
$ supersonic send --seed <32B> --pool pool.json --to 7mkdvPffx1GjMmj2oo3i8sYXADi1yA9J3AfgurBYqVWo \
      --amount 1000000 --k 4 --keypair id.json --rpc https://api.devnet.solana.com --broadcast
  bundle 1: K=4, 5000000 lamports moved
    leg 0: 1000000 -> 5HaKGqna…   (decoy)
    leg 1: 1000000 -> 7zqTQeT2…   (decoy)
    leg 2: 2000000 -> B9f2aG3P…   (decoy)
    leg 3: 1000000 -> 7mkdvPff…   (real)
  (operator only) real leg is at index 3        # to stderr, never in the observer view
  BROADCAST — signature: 5zd9D1V5…
```

Both the real leg (`7mkdv…`) and a decoy (`5HaK…`) show `0.001 SOL` on devnet afterward —
every leg settled, real and decoy alike, in one atomic tx. The live deployment also
enforces the invariants: a `send` with the real destination set to the signer was rejected
on chain with `SelfDestination` (Error 6004) before anything moved.

## 5. The whole thing, one command

```
$ cargo build-sbf --manifest-path programs/supersonic-tx/Cargo.toml   # e2e loads this .so
$ cargo test --workspace
    ... 54 passed; 0 failed
```

Five crates: `programs/supersonic-tx` (router), `supersonic-sdk` (amount + destination
layers), `supersonic-cli` (CLI), `dest-harness` (measurement), `e2e` (on-chain proof).
The 54 cover the amount layer's exchangeability, the destination pool's fail-closed
selection, **every program invariant** (`e2e/tests/program_invariants.rs` — each
`SupersonicError` and later-leg-revert atomicity), the SDK→program seam, and the CLI's
input parsing.

## 6. The residual it does NOT close — the funding graph, measured

The defended residual above is the ceiling of a **fully-warmed** pool on the *history*
channel. It does not close the **funding graph**: a decoy funded by the signer re-links to
them one hop out, so the leg whose destination was funded by *someone else* — the genuine
third-party payee — is the real one (DESIGN §4). We measured how big that residual is, and
found something about mainnet transfers worth stating on its own.

Method (`dest-harness/src/funding.rs` + the `funding-residual` bin, over an out-of-tree
mainnet collection of the same 749 history destinations): for each, find the first funder
of its destination and check whether it is the paying signer. A self-funded decoy always
has `funder == signer`; a third-party-funded real payee does not.

**The finding — most "transfer destinations" are not payees.** Only 200 of 749 (27%)
resolve to a first SOL funder at all. The other 73% are **transient token accounts** — we
sampled and classified them: **95% are already closed**, and the ones we dumped are
Associated Token Accounts for wrapped SOL, created + funded + `syncNative`'d + `closeAccount`'d
inside a single swap transaction (net SOL delta zero, so no "first funder" exists). These
are the sender's **own** swap plumbing, self-funded by construction — not third-party
payees. A self-funded decoy is therefore *indistinguishable* from the dominant destination
type on Solana. The funding-graph attack only bites against the minority that are durable,
third-party-funded wallets.

**The residual, scoped to that minority — genuine P2P payees:**

```
$ cargo run -p supersonic-dest-harness --bin funding-residual -- \
      --funding data/funding_study.jsonl --study data/dest_study.jsonl

  Resolved 200/749; third-party-funded among resolved: 173/200 = 86.5% (CI 81.1–90.6%)

  K |  residual  | 95% CI
  2 |   +0.274   | [+0.257, +0.287]
  8 |   +0.480   | [+0.450, +0.503]
 16 |   +0.514   | [+0.482, +0.538]
```

Among durable wallet destinations — the population a P2P transfer tool actually pays, and
the population its durable warmed decoys imitate — **86.5% were funded by a third party**,
giving a residual of **+0.27 (K=2) to +0.51 (K=16)**, nearly the size of the *open* history
channel (+0.317…+0.598). Scoped honestly:

- **For P2P payees, the funding residual is large** — closing the history channel buys
  little for the durable-payee case, exactly the §4 thesis.
- **Over all raw System transfers, most destinations are self-funded swap plumbing**, which
  a self-funded decoy matches — so the *average* leak is smaller, and the residual is a
  property of who you actually pay, not of the transfer population at large.

Either way the design point stands: no self-funded decoy scheme closes the third-party-payee
case — only decoys funded by unlinkable third parties (a crowd) can, the interface for which
is specified in `CHANNELS.md §5.1`. A full-population figure that resolves the transient accounts' wallet
funders needs per-account-type tracing (or an archival RPC); it would *lower* the average,
not raise the P2P-payee number.
