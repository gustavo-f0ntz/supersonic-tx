# supersonic-tx — a channel-complete decoy system

> Design doc, v0.3 — 2026-07-16. Target: PR to `solanabr/supersonic-tx` by 2026-07-28.
>
> **This is a standalone system, not a patch.** The org repo is empty (LICENSE only);
> `Jmkoygg`'s PR #1 is one proposal for what `supersonic-tx` should be, and this is
> another. We do not depend on his code. Where his math is correct — the exchangeable
> amount construction — we cite it as concurrent prior art and reimplement it with
> credit (§2.2). Our organizing principle is different and is the point: **a decoy is
> only believable if it matches the real leg in *every* observable channel, not just
> the amount** — and the channel that dominates in deployment is the one his harness
> deliberately holds constant.
>
> v0.1 (swap bundles) is retracted; the two measurements that killed it are in §7.

## 0. Thesis in one paragraph

An observer of a confirmed Solana bundle sees each leg's **amount** and each leg's
**destination**. Match only the first and the second gives you away. We measured the
destination channel on mainnet: **63.4% of real SOL transfer destinations already have
on-chain history** (n=1181, 95% CI 60.6–66.1%), while a derived decoy destination has
**none, by construction**. One `getSignaturesForAddress` per leg turns that gap into a
measured advantage of **+0.32 (K=2) to +0.60 (K=16)** over the `1/K` baseline —
roughly **50× the amount-channel advantage** PR #1 reports as its headline. This
system is built around closing that channel: a pre-warmed decoy pool whose history
profile is drawn from the same distribution as real payees, fail-closed when it can't
match, with the amount channel handled correctly (exchangeable construction, credited)
as table stakes. We measure the channel open and closed with one harness, and we state
the residual — the funding graph (§4) — that no self-funded decoy scheme can close.

## 1. The measurement (the original contribution)

### 1.1 Real destinations have history; derived decoys have none

Method: sample confirmed System-program `transfer` instructions from finalized mainnet
blocks; for each destination, count signatures **strictly before** the transfer's slot.

| n | had prior history | fresh |
|---|---|---|
| **1181** | **749 (63.4%)** | 432 (36.6%) |

95% Wilson CI on the 63.4%: **[60.6%, 66.1%]**.

**Honesty note — the estimate drifted down as n grew, and that matters.** A first n=25
probe read 92%; n=819 read 67.6%; n=1181 settled at 63.4%. The early probe was not
just imprecise, it was **outside the final CI** — it sampled a narrow slot range and
hit a high-history pocket. This is exactly why the kill gate demanded n≥1000, and it is
the number we build on. The thesis never depended on the point estimate: the kill gate
was "abort if < 0.30," and 0.634 clears it by a wide margin.

A derived decoy destination (`derive_decoy_keypair` in any such SDK) has prior history
**0, always** — the account does not exist until the bundle creates it. The two
populations do not overlap.

### 1.2 The resulting advantage — measured, not modelled

Harness (`dest-harness`) samples the real leg's destination profile from the empirical
study above and generates K−1 fresh decoys, then runs the destination classifiers,
selecting the best attack on train and scoring on test — PR #1's own discipline and
metric, so the numbers sit in one comparable table:

| K | PR #1 (amount channel) | **this channel (measured, n=1181)** | 95% CI | ratio |
|---|---|---|---|---|
| 2 | +0.037 | **+0.317** | [+0.309, +0.326] | ~9× |
| 4 | +0.027 | **+0.474** | [+0.464, +0.484] | ~18× |
| 8 | +0.013 | **+0.556** | [+0.545, +0.566] | ~43× |
| 16 | +0.012 | **+0.596** | [+0.586, +0.606] | **~50×** |

Note the opposite slopes: PR #1's amount advantage *falls* with K (+0.037 → +0.012);
this channel's *rises* (+0.317 → +0.596), because the `1/K` baseline collapses beneath a
near-perfect classifier. **"Use the largest K you can afford" is inverted here** —
raising K adds decoys the adversary discards for free.

Against PR #1's construction the four destination classifiers score **identically**
(all +0.596 at K=16): with decoys at zero history, *any* history feature separates them.
The attack is not clever; it is a single bit read with one RPC call per leg.

### 1.3 What the defense must reproduce

The `prior_sigs` distribution of real payees *with* history (median 914, mean 721;
78% hit the 1000-signature RPC page cap, so those counts are floors):

```
prior_sigs      share of all real destinations
        0        36.6%   ← the "fresh real payee" case; a matched pool must include it
      1-9         2.0%
    10-99         6.2%
  100-999        55.2%
    1000+         (>=1000, capped by the RPC page; ~78% of the 100-999 bucket are floors)
```

This is the target: a warmed pool must present this profile *distribution*, not a
single canonical "aged address." The 36.6% fresh bucket is load-bearing — it means a
real payment to a brand-new address is common, so a defense can leave *some* legs fresh
without leaking, which relaxes the pool requirement (§3.3).

Note on population (see §4): the study samples all System transfers, of which ~73% turn
out to be transient token accounts (swap plumbing). A fresh swap-ATA does not leak here
(no history, like a decoy), so the advantage above is measured over the *blended*
population and is **conservative for durable P2P payees** — the case this tool targets,
who almost all carry history. The advantage tracks the history share as `≈ P(history)·(1−1/K)`,
so at the blended P≈63% it is +0.60, while for durable payees (P→~100%) it is **+0.94 at
K=16**. The headline understates the leak for the real use case.

## 2. Prior art

### 2.1 Monero ran this experiment for eight years

| Year | Event | Lesson |
|---|---|---|
| 2017 | [Miller et al.](https://arxiv.org/pdf/1704.04299) — ring signatures traceable by **"guess-newest"** | RingCT hid amounts; the leak was **temporal**. Closing the value channel bought nothing while the age channel stayed open. |
| 2018 | v0.13 replaces uniform decoy sampling with a **gamma distribution** matched to real spend-age ([Möser et al.](https://arxiv.org/pdf/1812.02808)) | The fix is not more decoys — it is **matching the decoy distribution to the real one in the observable channel**. |
| 2023 | [monero#8872](https://github.com/monero-project/monero/issues/8872) — a decoy exactly 10 blocks old was unselectable; any ring with one → that member is the real spend | **One unmatched moment in one channel collapses the whole set.** |

The destination channel is the 2017 state, verbatim: decoys from one distribution
(fresh keys, history 0), the real from another (existing addresses, median 914 sigs).
Monero's version was probabilistic; ours is **binary and measured at 63%** — the class
of bug that made Monero traceable.

### 2.2 supersonic-tx PR #1 (@Jmkoygg) — concurrent, credited

PR #1 is strong work: an honest threat model, and an **exchangeable amount construction**
that treats the real amount as one draw from the decoys' own log-normal
(`mu = ln(real) − sigma·z_real`) so it is neither the most central nor the most extreme
value. That is the correct amount-channel defense — the 2018 gamma fix applied to value.
**We reimplement it, with credit, as our amount layer** (~150 lines); we do not improve
on it and do not pretend to. Our contribution is orthogonal: the *destination* channel,
originally held constant in his harness —

> "in the harness every destination (real and decoy) is a fresh key, so the destination
> channel is held constant... defended operationally by pre-warming decoy addresses /
> a companion account-cooker, not something this harness claims to measure."
> — `harness/src/classifiers.rs`, PR #1 at the time this system was designed

**Updated as of his later commits.** PR #1 has since added a harness-level *model* of
this channel plus a small self-collected devnet fixture (18 addresses,
`eval_history_measured`) — and is candid about its scope, in his own words: the
pre-warmed regime's ~0 advantage "follows from the sampling construction itself... not a
property discovered in the devnet data," and the fixture does "not validate that a real
account-cooker's warming pattern... is itself indistinguishable from organic activity"
(`harness/src/destination.rs`). His shipped SDK (`sdk/src/lib.rs::derive_decoy_keypair`)
still derives a fresh key for every decoy — the model lives in the harness, not in the
tool a caller runs. Two differences remain, both load-bearing: (1) his fixture is 18
self-collected devnet addresses standing in for both "real payee" and "warmed decoy" at
once; ours is **n=1181 independently-sampled real mainnet transfers** — a population, not
a stand-in. (2) his model stops at history existence; ours ships the selection code
itself (`WarmingPool::select`, fail-closed on `PoolTooCold`/`PoolNotRepresentative`) as
the path a caller actually invokes, and separately measures a channel he has not
addressed at all — the funding graph (§4), where the residual is comparable in size to
the open history channel itself. Our generalization of PR #1's own idea stands: he made
the real *amount* exchangeable with the decoys'; we make the real *destination profile*
exchangeable with the decoys', ship it as the default path, and measure what remains open
beyond it.

## 3. Design — a standalone channel-complete system

Four crates, all ours:

```
programs/supersonic-tx   atomic K-leg transfer router (I1–I4), our own
supersonic-sdk           bundle planning + amount layer (exchangeable, credited)
                         + destination layer (matched pre-warmed pool) ← the spine
supersonic (cli)         plan / warm / send / recover / inspect
dest-harness             destination-channel adversary + advantage measurement (done)
```

### 3.1 The amount layer — table stakes, credited

Exchangeable log-normal construction per §2.2, reimplemented with attribution. It is
correct and small; we spend no design budget improving it and every reader is pointed at
PR #1 as the origin.

### 3.2 The destination layer — the spine

Profile-matched pre-warmed pool. Per the Monero lesson, the decoy history profile must
be exchangeable with the real destination's over `(age, tx_count, recency)`.

- **Warming pool.** The SDK maintains derived addresses created and exercised on a
  schedule that reproduces the §1.3 empirical distribution.
- **Selection, not generation.** You cannot synthesise six months of age on demand; you
  select warmed members that make the real leg a non-outlier in every profile dimension.
- **Fail closed.** No matched set for this real destination → the SDK **refuses to build
  the bundle** rather than emit one that leaks. Silent degradation to +0.6 advantage is
  worse than a refusal.

### 3.3 The relaxation the data buys us

Because 36.6% of real payments go to fresh addresses (§1.3), a bundle may leave a
matched *fraction* of legs fresh without leaking — the real leg among fresh legs is
plausible. The pool need not be enormous; it needs to reproduce the distribution,
including its fresh tail. This is a direct, measured design consequence, not an
assumption.

### 3.4 Invariants (program)

- **I1 Non-custodial** — no pool/escrow/PDA holds funds; each leg moves the signer's own
  lamports to a destination the signer supplied, in one atomic tx. Keeps us off the
  money-transmitter line (the PR #1 §7 reasoning is sound and we adopt the same posture).
- **I3 Atomicity** — all legs or none; a partial landing that exposes the real leg
  without its decoys defeats the tool.
- **I4 Fail-closed** — malformed leg reverts the bundle.

## 4. Where this stops — the funding graph

Matching `(age, tx_count, recency)` closes history-existence. It does not close
**provenance**. A decoy destination must be (a) user-controlled, to be recoverable;
(b) profile-matched; and (c) not traceably the user's. **(a) and (c) are in tension** —
every lamport that warmed a decoy came from somewhere, and if it traces to the signer,
an adversary who walks the funding graph re-identifies the decoys and the leg that
doesn't trace back is the real one. Breaking that link is mixing, which the I1 posture
forbids by design.

**Measured** (PROOF §6), with a twist worth stating: tracing 749 real transfer
destinations, only 27% resolve to a SOL funder — the other 73% are **transient token
accounts** (95% now closed; wrapped-SOL ATAs created and closed inside one swap tx), the
sender's own self-funded plumbing, which a self-funded decoy already matches. Among the
durable-wallet minority — genuine P2P payees, what this tool actually pays — **86.5% were
third-party-funded**, a residual of **+0.27 (K=2) to +0.51 (K=16)**, nearly the size of the
open history channel. So for P2P payees, closing history buys little while the funding graph
stays open — the point of this section, now with a number and an honest scope.

Closing it needs decoys funded by unlinkable third parties — a crowd — which no
self-funded scheme can synthesize (the interface for that is specified in `CHANNELS.md
§5.1`).

Our honest scope: **close the history channel, measure the funding residual, and specify
the crowd interface that would close it.** We do not claim to close what self-funded decoys
cannot. Overclaiming is the failure PR #1 spent fifteen commits walking back; we will not
repeat it.

## 5. Deliverables

1. `dest-harness`: destination-channel adversary + advantage table vs the fresh-decoy
   construction. **Done** — n=1181 study, measured advantage +0.317…+0.596, 16 tests pass.
2. The empirical mainnet payee-profile dataset (`dest-harness/data/dest_study.jsonl`) —
   a public artifact the ecosystem does not have.
3. Standalone program + SDK (amount layer credited, destination layer new) + CLI.
4. `PROOF.md` with the harness table open→closed, `CHANNELS.md` with the destination
   channel measured and the funding-graph residual + crowd interface stated as open.

## 6. Results — the channel, open and closed (measured, n=1181)

Decoys from a warmed pool whose profile distribution reproduces the real-payee
population (fit on the train split); real legs from the held-out test split, so the
closure is generalization, not decoys drawn from the same rows as the reals.

| K | open (fresh decoys, PR #1) | **defended (warmed pool)** |
|---|---|---|
| 2 | +0.317 | **−0.001** |
| 4 | +0.476 | **−0.002** |
| 8 | +0.550 | **+0.010** |
| 16 | +0.598 | **+0.007** |

The defended column is produced by drawing decoys through the **deployed
`WarmingPool::select`** path (matured pool reproducing the train distribution, fresh-share
gate and all), so the number reflects the code a user runs — not a model standing in for it
(`select_path_closes_the_channel` pins the same property in CI). The residual (~+0.01,
+0.007 at K=16) is the same order as PR #1's amount-channel residual (+0.012): both
channels close to a small measured floor, not a suspicious exact zero. Against a
nonlinear ensemble instead of the best single or linear attack, the honest floor is
larger — +0.09 at K=16 (`PROOF.md §2.3`), still 4–7× below the open channel at every K,
and explained there rather than only reported. The funding-graph residual (§4) is
untouched by profile matching and remains measured-open.

## 7. Retracted theses (v0.1) and the standalone decision

**Swap bundles — dead.** Six real Jupiter v6 mainnet swaps: 74k–149k CU (ceiling 1.4M —
fine) but **27–31 account locks each against a `MAX_TX_ACCOUNT_LOCKS = 64` cap → K_max =
2**, a coin flip. Account locks bind, not CU. The v0.1 CU hypothesis was wrong.

**Copy-poisoning via decoy swaps — dead.** Diluting your alpha across K legs dilutes a
copy-trader's by the same factor; the ratio is unchanged, and you paid K× slippage for
it. **Your position *is* the signal** — you cannot emit a position you do not hold.
Transfers escape this because a transfer to your own address is an economic no-op.
**This is why the transfer scope is correct**, and the v0.1 claim that PR #1 "solved the
wrong problem" was wrong.

**Why standalone, not a patch on PR #1.** Contributing *into* his PR frames him as the
base and us as a secondary edit — and on raw volume he wins (10.7k lines). But his
reusable surface is tiny: the program is a ~75-line transfer loop, the amount math is
~150 lines, and everything else (harness, CLI, docs) we write our own anyway because
ours must be destination-aware from the origin. Building standalone costs ~150 lines of
credited reimplementation and buys full ownership of the system a judge evaluates. We
are a peer proposal whose spine is the channel he left open — not a review of his.
