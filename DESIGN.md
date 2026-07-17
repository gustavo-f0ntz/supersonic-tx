# supersonic-tx — closing the destination-history channel

> Design doc, v0.2 — 2026-07-16. Target: PR to `solanabr/supersonic-tx` by 2026-07-28.
> Downstream of `Jmkoygg`'s `THREAT_MODEL.md` (PR #1). We adopt his metric, his harness
> methodology, and his exchangeable construction unchanged. This document attacks the
> one channel his own threat model flags as open and his harness explicitly does not
> measure.
>
> **v0.1 is retracted.** It proposed moving the construction from transfers to swaps.
> That thesis is dead; §7 records why, because the reasoning is load-bearing for this
> one.

## 0. Thesis in one paragraph

PR #1 measures the **amount** channel honestly and closes it well (advantage +0.013 at
K=8). But a bundle's destinations are also observable, and PR #1's decoy destinations
are freshly-derived keys with **zero on-chain history**, while a real payment
destination almost always has one. We measured it: **92% of real mainnet SOL transfer
destinations have prior on-chain history** (median 974 prior signatures). One RPC call
per leg therefore identifies the real leg at ≈0.93 — an advantage of **+0.805 at K=8,
roughly 62× the channel PR #1 reports as its headline result**. This is the 2017 Monero
mistake with a harder edge: Monero's "guess-newest" was probabilistic; "guess the
address that exists" is binary. This document measures the leak, builds the best
available defense (a pre-warmed decoy pool with a history profile matched to real
counterparties — the 2018 Monero gamma fix, ported to Solana), and states precisely
where that defense stops and why.

## 1. The measurement (this is the contribution)

### 1.1 Real destinations have history; decoys have none

Method: sample confirmed System-program `transfer` instructions from finalized mainnet
blocks; for each destination, count signatures **strictly before** the transfer's slot.

| | n | had prior history | fresh |
|---|---|---|---|
| Real mainnet transfer destinations | 25 | **23 (92%)** | 2 (8%) |

Prior-signature count among those with history: **median 974**, min 9, max 999 (the
query cap — several are effectively unbounded).

> **Honesty note.** n=25 is a probe, not a study. Wilson 95% CI on 23/25 is roughly
> [0.75, 0.98]. Day 1 scales this to n≥1000 across a slot range and reports the CI.
> The thesis does not depend on the point estimate being exactly 0.92 — anything above
> ~0.3 makes this the dominant channel.

PR #1's decoy destinations, by construction (`sdk/src/lib.rs::derive_decoy_keypair`),
are fresh Ed25519 keys. Prior history: **0, always**. The two distributions do not
overlap.

### 1.2 The resulting advantage

Classifier: *"the real leg is the one whose destination already existed."*

```
P[identify] = P(real dest has history) × 1  +  P(real dest is fresh) × (1/K)
            = 0.92 + 0.08/K
advantage   = P[identify] − 1/K
```

| K | PR #1 measured (amount channel) | this channel (measured basis) | ratio |
|---|---|---|---|
| 2 | +0.037 | **+0.46** | 12× |
| 4 | +0.027 | **+0.69** | 26× |
| 8 | +0.013 | **+0.805** | **62×** |
| 16 | +0.012 | **+0.863** | **72×** |

Note the direction: PR #1's amount-channel advantage *improves* with K (+0.037 → +0.012)
while the destination channel *worsens* (+0.46 → +0.863), because the 1/K baseline falls
away beneath a classifier that is already near-perfect. **PR #1's headline advice — "use
the largest K you can afford" — is inverted on this channel.** Raising K buys amount
ambiguity and buys nothing here; it only adds decoys the adversary discards for free.

### 1.3 PR #1 is honest about this, and that matters

`harness/src/classifiers.rs`, header:

> "Destinations are deliberately *not* a feature here: in the harness every destination
> (real and decoy) is a fresh key, so the destination channel is held constant to
> isolate the amount/position channel the SDK actually controls. The destination-history
> channel is a separate, acknowledged attack (defended operationally by pre-warming
> decoy addresses / a companion account-cooker), not something this harness claims to
> measure."

Holding a channel constant is correct experimental method — it isolates the variable
under test, and the amount-channel result is valid *given* that control. It is not a
defense: in deployment the channel is not held constant, and neither cited mitigation
exists. `pre-warming` is absent from the SDK; the `account-cooker` sibling repo is at
one commit of scaffold. **We are not contradicting PR #1. We are measuring the thing it
told us it had not measured.**

## 2. Prior art — Monero ran this experiment for eight years

| Year | Event | Lesson |
|---|---|---|
| 2017 | [Miller et al.](https://arxiv.org/pdf/1704.04299) — a large share of ring signatures traceable by **"guess-newest"** | RingCT had already hidden amounts. The leak was **temporal**. Closing the value channel bought nothing while the age channel stayed open. |
| Oct 2018 | v0.13 replaces uniform decoy sampling with a **gamma distribution** matched to the real spend-age distribution ([Möser et al.](https://arxiv.org/pdf/1812.02808)) | The fix is not more decoys. It is **matching the decoy distribution to the real one in the observable channel**. |
| 2023 | [monero#8872](https://github.com/monero-project/monero/issues/8872) — off-by-one made a decoy exactly 10 blocks old unselectable; any ring containing one → that member is the real spend | **One unmatched moment in one channel collapses the whole set.** |

The mapping is exact:

- PR #1's exchangeable construction **is** the 2018 gamma fix, correctly applied to the
  amount channel. It is good work and we inherit it verbatim.
- The destination channel **is** the 2017 state: decoys drawn from one distribution
  (fresh keys, age 0, history 0), the real drawn from another (existing addresses,
  median 974 sigs). Monero's version was probabilistic. Ours is **binary and
  measured at 92%** — worse than the bug that made Monero traceable.

## 3. Design — `dest-history` adversary + matched decoy pool

### 3.1 The adversary (ships first, day 1)

A new harness classifier family over the destination channel, run against **PR #1's
unmodified construction**, with the number published:

- `dest_exists` — did the account exist before the bundle slot? (binary, the 92% attack)
- `dest_age` — slots since first signature
- `dest_txcount` — prior signature count
- `dest_recency` — slots since last activity before the bundle
- `dest_funding` — does the destination's funding trace to the signer? (see §4)
- a learned adversary over the union, same train/test discipline PR #1 established

This runs against real chain state, not a simulation. That is the difference from PR
#1's harness and the reason the number is credible.

### 3.2 The defense — profile-matched pre-warmed pool

Per the Monero lesson, the decoy history profile must be **exchangeable** with the
real destination's. Note this is precisely PR #1's own insight, generalized: he made the
real *amount* one draw from the decoys' distribution; we make the real *destination
profile* one draw from the decoys' distribution. Same construction, harder variable.

Profile vector per destination: `(age, tx_count, recency, balance, owner_program)`.

- **Warming pool.** The SDK maintains N derived addresses created and exercised on a
  schedule that reproduces the empirical profile distribution of real payees (measured
  in §1, scaled up on day 1).
- **Selection, not sampling.** Unlike amounts, you cannot *generate* an address with six
  months of age on demand. You select from what you warmed. Bundle-time selection is a
  matching problem: given the real destination's profile, choose K−1 pool members such
  that the real is not an outlier in any profile dimension.
- **Fail closed.** If the pool cannot produce a matched set for this real destination,
  the SDK **refuses to build the bundle** rather than emitting one that leaks. A tool
  that silently degrades to 0.8 advantage is worse than one that says no.

### 3.3 Cost, stated plainly

Warming costs fees and, more importantly, **time**: an address with a plausible age
distribution had to be created months ago. **Privacy has a lead time.** A user who
installs the tool today has no aged pool and cannot be defended today. This is not an
implementation gap to apologize for — it is the finding: behavioral privacy requires
pre-committed capital and time, and any tool claiming otherwise is lying.

## 4. Where this defense stops — the funding graph (read before day 2)

Matching `(age, tx_count, recency)` closes the *history-existence* attack. It does not
close **provenance**.

A decoy destination must be (a) controlled by the user, to be recoverable — PR #1's I2;
(b) profile-matched to a real payee; and (c) not traceably the user's. **(a) and (c) are
in tension.** Every lamport that warmed a decoy address came from somewhere. If it
traces to the signer, an adversary who walks the funding graph re-identifies the decoys
as self-owned, and the leg that *doesn't* trace back — the genuine third-party payee —
is the real one. The leak returns, one hop out.

Breaking that link requires severing your own funding provenance, which is mixing, which
is precisely what PR #1's §7 legal posture forbids by design (and rightly so).

This is the circular dependency at the centre of the whole bounty, and it should be
stated out loud:

- **supersonic-tx**'s dominant leak closes only with aged, plausibly-funded decoy
  identities → that is **account-cooker**'s job.
- **account-cooker**'s identities are self-funded, so the funding graph re-links them →
  they do not form an anonymity set.
- A crowd that actually closes it must be made of **other people's** activity → that is
  **mirror-pool**.

So the honest scope of this PR: **we close the history channel, we measure the residual
funding channel, and we define the interface a crowd must implement to close it.** We do
not claim to close what cannot be closed with self-funded decoys. Claiming otherwise is
the failure mode PR #1 spent fifteen commits walking back, and we will not repeat it.

## 5. Deliverables

1. `swap-harness` → `dest-harness`: the destination-channel adversary, run against PR
   #1's construction, number published with tx-level evidence. **This alone is the PR's
   main value.**
2. The n≥1000 empirical study of real payee profile distributions on mainnet (a public
   dataset the ecosystem does not have).
3. Profile-matched pool + selection algorithm + fail-closed integration in the SDK.
4. `THREAT_MODEL` §4.7 proposed upstream: the destination channel, measured, with the
   funding-graph residual stated as an open problem and the account-cooker/mirror-pool
   interface specified.

## 6. Four-day plan

| Day | Deliverable | Gate |
|---|---|---|
| 1 | Scale §1 to n≥1000 with CI. Ship `dest_exists`/`dest_age`/`dest_txcount` adversary against PR #1's construction. Publish the advantage table. | **Kill gate:** if P(real dest has history) < 0.3 at scale, the leak is not dominant — say so publicly and stop. |
| 2 | Profile model + warming schedule + selection algorithm. Measure the funding-graph residual. | Matched selection drives `dest_exists` advantage → ~0. |
| 3 | SDK integration, fail-closed path, litesvm tests. | End-to-end bundle with a matched pool. |
| 4 | `PROOF.md`, PR, upstream §4.7, honest limits (cold start, funding graph). | Advantage table published, including where we still fail. |

## 7. Retracted: the swap thesis (v0.1)

v0.1 proposed moving from transfers to swap bundles. Two measurements killed it in one
afternoon; both are recorded because they are the reason to trust v0.2.

**Account locks, not compute units, cap swap bundling.** Six real Jupiter v6 mainnet
swaps: 74k–149k CU (ceiling 1.4M — comfortable) but **27–31 account locks each, against
a `MAX_TX_ACCOUNT_LOCKS = 64` hard cap**. K=2 fits at 91% of the cap; K=3 needs ~136%.
**K_max = 2** for venue-diverse swap bundles in one transaction, and K=2 is a coin flip.
The CU hypothesis in v0.1 was simply wrong.

**Swap decoys cannot work, for a deeper reason.** You have 100 SOL and alpha in BONK.
Poisoned K=4, you buy 25 each of BONK/WIF/JUP/PEPE; a copy-trader mirrors 25 each. Your
alpha exposure: 25. Theirs: 25. **The ratio is unchanged** — you diluted yourself exactly
as much as them, and paid 3× slippage for it. There is no wedge, because **your position
*is* the signal**: you cannot emit a position you do not hold. Restoring your real
exposure requires concentrating, which is public, which they follow.

Transfers escape this precisely because a transfer to an address you control is not a
position — it is an economic no-op that costs a fee. **This is why PR #1's scope choice
is correct, and the v0.1 claim that it "solved the wrong problem" was wrong.** Decoys are
economically coherent only where they are free to hold. That is transfers, and it is not
swaps.
