# CHANNELS — supersonic-tx threat model, channel by channel

> What this system defends, against whom, and — stated as plainly as the wins — where it
> stops. Measurements are in `PROOF.md`; design rationale in `DESIGN.md`. This document
> is the honest boundary: a decoy scheme that hides where it fails is worse than none.

## 1. What the tool does

A user moves lamports to a real destination. supersonic-tx builds a single atomic
transaction with **K legs** — one real, K−1 decoys — so an on-chain observer cannot tell
which leg is the real payment. Decoys are the user's own recoverable addresses; the
transaction is non-custodial (no pool, escrow, or PDA ever holds funds).

## 2. Adversary

A passive on-chain observer with full history and unlimited RPC. They see every confirmed
transaction and can, for any account, call `getSignaturesForAddress`, read balances, and
walk the funding graph backward. They do **not** control the user's keys, the RPC endpoint,
or the validator, and they cannot see off-chain state (which leg the user "meant").

Goal: given a confirmed K-leg bundle, identify the real leg with probability better than
the `1/K` baseline. The metric is **advantage = P[identify] − 1/K**, adopted unchanged
from PR #1 so results are comparable.

## 3. Observable channels and their status

Each leg exposes an **amount** and a **destination**. A believable decoy must match the
real leg in *every* observable channel, not just one.

| Channel | Observable | Status | Evidence |
|---|---|---|---|
| **Amount** | lamports per leg | **Closed** (credited reimpl of PR #1) | +0.037→+0.012, PR #1's harness |
| **Destination — history existence** | does the account exist before the bundle slot? | **Closed** (this system's contribution) | open +0.317→+0.598 → defended −0.001→+0.007 (best single attack), +0.09 worst-case against a nonlinear ensemble, via `WarmingPool::select` (`PROOF.md §2`, §2.3) |
| **Destination — age / txcount / recency** | profile of prior activity | **Closed** — same warmed-pool matching | all classifiers +0.601 open → ~+0.01 defended |
| **Atomicity / partial landing** | did only some legs settle? | **Closed** (program invariant I3) | `underfunded_sdk_bundle_reverts_atomically` (`PROOF.md §4`) |
| **Destination — funding provenance** | does the decoy's funding trace to the signer? | **OPEN — measured +0.27…+0.51 for durable P2P payees (most destinations are self-funded plumbing decoys already match); not solvable by self-funded decoys** | §5 below |
| **Destination — token holdings** | does the leg's address hold any SPL token account? | **OPEN — measured +0.05…+0.09; a SOL-only warming scheme can't hold tokens by construction** | §5.2 below |
| **Program identity** | did this signer ever invoke supersonic-tx at all? | **OPEN — measured live (1 real tx → 1 signer recovered, zero bundle access needed); closing it costs composability** | §7 below |
| **Timing / co-signing / same-tx correlation** | all legs share one tx and signer | Inherent to the construction; out of scope | §8 |

### 3.1 Amount channel — table stakes, credited

The real amount is treated as one draw from the decoys' own log-normal
(`mu = ln(real) − sigma·z_real`), so it is neither the most central nor the most extreme
value. This is PR #1's (@Jmkoygg) exchangeable construction, the correct amount-channel
defense; we reimplement it with credit (`supersonic-sdk/src/amounts.rs`, ~150 lines) and
do not claim to improve it.

### 3.2 Destination channel — the spine

PR #1's decoys are fresh keys with zero history; real payees usually have history (63.4%,
n=1181). One RPC call per leg reads that bit. The defense is a **profile-matched
pre-warmed pool**: derived addresses created and exercised on a schedule that reproduces
the empirical real-payee profile distribution (`age, tx_count, recency`), so any real leg
is one more i.i.d. draw from the decoys' distribution — the 2018 Monero gamma lesson,
ported to Solana. Exchangeability is **global, not per-bundle**: we do not match decoys
"near" the real leg (that recreates a centrality leak); we make the whole pool reproduce
the population.

The defense **fails closed**. `plan_bundle` refuses rather than emit a leaking bundle:
`PoolTooCold` (too few mature members) and `PoolNotRepresentative` (matured members don't
reproduce the fresh/history split within `FRESH_SHARE_TOL = 0.15`). Privacy has a lead
time — an aged pool had to be warmed months ago — and a user with no warmed pool is told
no, not silently degraded.

That fail-closed check is only as honest as the `current` profile it reads. `warm` alone
never touches the network — `--mature` fakes maturity for a demo, and otherwise `current`
is whatever it was last set to, with nothing re-checking it against reality.
`supersonic refresh` closes that: it re-queries `getSignaturesForAddress` for every pool
member and overwrites `current` with what's actually observed, so eligibility is checked
against the chain, not a locally-asserted flag (`AUDIT.md`).

## 4. Program invariants (trust boundary)

- **I1 Non-custodial** — every leg moves the signer's own lamports to a destination the
  signer supplied, in one atomic tx. No component holds user funds. Keeps the tool off the
  money-transmitter line (PR #1's §7 posture, adopted).
- **I2 Recoverable decoys** — every decoy destination is `derive_pool_keypair(seed, index)`;
  the user can sweep the funds back. Proven in e2e by re-deriving each leg's address.
- **I3 Atomicity** — all legs land or none. A partial landing that exposes the real leg
  without its decoys is the failure mode; the program reverts the whole bundle on any
  malformed or underfunded leg.
- **I4 Fail-closed** — malformed input reverts rather than degrading.

## 5. The residual — funding provenance, measured (§4 of DESIGN.md)

Matching `(age, tx_count, recency)` closes history-*existence*. It does **not** close
**provenance**. A decoy destination must be (a) user-controlled, to be recoverable (I2);
(b) profile-matched (§3.2); and (c) not traceably the user's. **(a) and (c) are in
tension.** Every lamport that warmed a decoy came from somewhere; if it traces to the
signer, an adversary who walks the funding graph re-identifies the decoys as self-owned,
and the one leg that *doesn't* trace back — the genuine third-party payee — is the real
one. The leak returns one hop out.

**How big is this residual? We measured it, and hit a finding worth stating** (`PROOF.md
§6`). Tracing the first funder of 749 real transfer destinations: only 27% resolve to a SOL
funder at all — the other 73% are **transient token accounts** (95% now closed; the ones we
dumped are wrapped-SOL ATAs created + funded + closed inside one swap tx). Most mainnet
"transfer destinations" are the sender's **own** swap plumbing, self-funded by construction
— so a self-funded decoy is *indistinguishable* from the dominant destination type. The
attack bites only against the minority that are durable, third-party-funded wallets — and
there it bites hard: **86.5% of durable payees were third-party-funded** (CI 81.1–90.6%),
residual **+0.27 (K=2) to +0.51 (K=16)**, nearly the size of the open history channel.

So the honest scoping: for **genuine P2P payees** — what a transfer tool pays, and what its
durable warmed decoys imitate — the funding residual is large, exactly the §4 thesis. Over
*all* raw transfers the average leak is smaller, because most destinations are self-funded
plumbing the decoys already match. The residual is a property of who you pay, not of the
transfer population at large.

Breaking the link means severing your own funding provenance, which is **mixing**, which
the I1 non-custodial posture forbids by design. So the third-party-payee residual cannot be
closed by any self-funded decoy scheme, including this one. We close the history channel,
**measure** the funding residual, and do **not** claim to close it.
(Overclaiming here is the exact failure PR #1 spent fifteen commits walking back.)

### 5.1 What would close it — the crowd interface

Closing this residual requires decoy destinations whose funding provenance is **not** the
signer's: addresses drawn from a set of mutually unlinkable, **externally-funded**
participants. That is a *crowd*, and no single self-funded actor can synthesize one — which
is precisely why supersonic-tx cannot close the residual alone and does not claim to.

So supersonic-tx defines the **interface**, not the crowd: decoy destinations presented to
`plan_bundle` in place of self-derived pool members, funded by unlinkable third parties,
while preserving I1/I2 (the user must still prove no custodial hold and recover any
misdirected funds). A complementary source of externally-funded, plausibly-aged identities
would slot in here; supplying that source is a separate problem from the one this tool
solves.

### 5.2 A second residual, found by looking for more: token holdings

The funding residual isn't the only thing a SOL-only warming scheme misses. A pool
member is aged **exclusively** by `WarmingPool` moving lamports to it — it never touches
the SPL Token program, so it holds **zero token accounts, always**. A real payee is
under no such constraint.

**Measured** (`data/token_holdings.jsonl`, `getTokenAccountsByOwner` against the classic
SPL Token program for the same 749 history-having destinations `dest_study.jsonl`
tracks; Token-2022 not queried — a scope limit stated, not hidden): **111/749 = 14.8%**
of history payees hold ≥1 token account (95% CI 12.5–17.5%), i.e. **9.4%** over the full
1181-destination population. Same shape as every other channel here — a decoy is always
the "no tokens" case, so the attack reads off the unique leg that holds tokens when the
real one does, and falls back to `1/K` when it doesn't:

```
$ cargo run -p supersonic-dest-harness --bin token-residual -- \
      --tokens data/token_holdings.jsonl --study data/dest_study.jsonl

  K | residual advantage | 95% CI
  2 |        +0.047       | [+0.039, +0.056]
  4 |        +0.070       | [+0.059, +0.083]
  8 |        +0.082       | [+0.069, +0.097]
 16 |        +0.088       | [+0.074, +0.104]
```

Smaller than the funding residual, but real, and it closes the same way: decoys that
also plausibly hold tokens — exactly what a real `account-cooker` casting swap/stake
activity through decoy addresses (README's composability section) would supply as a
side effect, not something this SOL-only scheme can synthesize alone. §5.1's crowd
interface is the same interface this channel needs, not a second one.

### 5.3 A third residual: cross-bundle identity reuse

Every number above is a **single-bundle** measurement: given one confirmed bundle, can
an observer spot the real leg? PR #1's own `92ee379`→`a8f7b2f` commits found a sharper
question for their per-slot sub-funder mitigation — a fix that drives a *single* bundle's
residual to ~0 can still fail once an observer has watched *many* bundles from the same
signer, because a mitigation built on a small, reused, finite set of identities eventually
lets those identities be told apart from a genuinely one-time real payee just by which ones
recur. They measured this for their funding-graph fix and reported it straight: the
sub-funder mitigation's advantage climbs back to the pre-mitigation ceiling
(+0.75/+0.875/+0.9375 at K=4/8/16) by bundle 25-50 of normal pool reuse.

The same question applies to `WarmingPool` itself, independent of funding entirely: a
decoy's identity *is* its pool member index, and `select` draws from a fixed pool (32
members by the CLI's `warm --count` default). An observer who has watched N bundles from
one signer can tally how many times each destination address has appeared before —
directly, on-chain, no funding trace needed — and guess "least-seen-so-far = real" on
bundle N+1. Measured (`dest-harness/src/cross_bundle.rs`, real `WarmingPool::select` path,
n=1181 mainnet study, 300-trial means):

```
$ cargo run -p supersonic-dest-harness --release --bin cross-bundle-residual -- \
      --study data/dest_study.jsonl --pool-size 32 --k 8,16

  bundles observed |  advantage, K=8  |  advantage, K=16
  ------------------+------------------+------------------
                  5 |      +0.098      |      +0.207
                 10 |      +0.259      |      +0.511
                 25 |      +0.576      |      +0.765
                 50 |      +0.725      |      +0.851
                100 |      +0.800      |      +0.894
```

By bundle 25, this alone exceeds the open destination-history channel's own ceiling
(+0.60 at K=16) — a defended pool that looks closed in isolation degrades to worse than
undefended within the first few dozen uses if the pool never grows.

**Verified this is not implementation-specific.** Jmkoygg's own `WarmPool` decoy mode
(`sdk/src/warming.rs::select_pool_slots` + `derive_pool_member_keypair`) uses the same
CLI default (pool size 32, K=8) and the identical construction — a bundle-seeded random
subset of a fixed pool. Running the same attacker directly against his real functions
(not a reimplementation) produces an essentially identical curve (+0.093/+0.262/+0.579/
+0.727 at bundles 5/10/25/50) — confirming this is a shared property of *any* finite,
self-warmed, reused decoy pool, independent of which side built it.

**A fix was tried, measured, and reverted before shipping — the honesty this document
exists for.** The natural idea — bias `select` toward whichever members have been drawn
least so far ("least-used-first", spreading usage evenly instead of leaving it to chance)
— was implemented and benchmarked. It made every checkpoint past bundle ~5 *worse*, not
better: the attacker's signal here is binary (has this address ever been seen before, yes
or no), not frequency-graded, and least-used-first saturates the *entire* pool (every
member drawn at least once) far faster and more evenly than a uniform independent draw
does. Reaching "every decoy has been seen at least once, while the real leg — a genuinely
one-time identity — never has" sooner is strictly worse for the defender. This is the same
methodological discipline as the depth-sweep forest finding (`PROOF.md §2.3`) and Gap A/B
(`DESIGN.md` devlog): measure before shipping a fix, and report the number even when it
contradicts the intuition that motivated the attempt.

**Why this is structural, not a bug to patch.** The root cause is the same one §5/§5.1
name for the funding-graph residual: a self-warmed pool is finite, and any finite set of
reused identities is eventually distinguishable from a population of genuinely one-time
real payees, independent of the order they're drawn in. Closing it needs the same fix
already specified — an externally-supplied, effectively unbounded decoy source (§5.1's
crowd interface) — not a cleverer `select`. Until then, the practical mitigation is
operational, not algorithmic: keep the pool large relative to expected bundle volume (this
measurement's own numbers are the sizing guide — a K=8 user planning to send >10 bundles
before rotating pools needs meaningfully more than 32 members) and rotate in freshly-warmed
members before the old ones saturate. Stated open, like §5 and §5.2 — not closed by this
submission.

## 6. Pre-inclusion (mempool) vs. post-hoc analysis

Worth stating plainly, because the two threats are usually named together and on Solana
they are not symmetric: **Solana has no public mempool.** Transactions are forwarded by
RPC providers straight to the current and next slot leaders rather than gossiped to a
public pending pool, so there is no shared queue an observer can subscribe to and watch
bundles before they land. The pre-inclusion surface that exists is *privileged*, not
public: the leader, and whichever RPC provider you submitted through, see the transaction
early — which is a trust question about your submission path, not a channel a passive
observer reads.

That is why this threat model puts a passive **post-hoc** observer (§2) at the center. It
is the adversary that actually scales: unlimited, retroactive, and available to anyone with
an RPC key and the confirmed ledger — which is precisely what modern chain-analysis and
copy-trading run on. A bundle is a single atomic transaction, so it either lands whole or
not at all; there is no partial pre-inclusion state to leak, and the K legs become visible
at the same instant. Every number in `PROOF.md` is measured against that observer.

Out of scope on the pre-inclusion side, therefore: a **malicious leader or RPC provider**
that front-runs on early sight of the bundle. Ambiguity still holds against them — they
read the same K indistinguishable legs everyone else does, just sooner — but they can drop
or reorder, and this construction does not defend against that.

## 7. Program identity — a channel outside the K-anonymity model

Every measurement above answers one question: *given a confirmed bundle, which leg is
real?* There is a different, earlier question this system does not answer: **did this
signer ever use supersonic-tx at all?**

The program is deployed at one fixed, known address
(`D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be`). `getSignaturesForAddress` on a
*program* returns every transaction that ever invoked it — every signer who has ever
cast a bundle through this deployment is enumerable by anyone, with **zero** access to
any bundle's contents. This is categorically different from every channel in §3: it
doesn't touch which leg is real, it identifies *that you use a privacy tool at all* —
the same property that makes a deployed Tornado Cash-style mixer observable as "this
wallet interacted with the mixer," independent of what it hid inside.

**Measured, live** (`cli/src/bin/program_identity.rs`,
`cargo run -p supersonic-cli --bin program-identity -- --rpc <url>`). Querying this
project's own devnet deployment initially returned **0** historical signatures — not
because nothing was ever sent (`README.md`/`PROOF.md` cite an earlier settled bundle),
but because `api.devnet.solana.com`'s public indexer does not retain signature history
indefinitely (the same shallow-index limitation already documented for mainnet RPCs in
`PROOF.md §6`, here on devnet instead). Rather than assert the mechanism from a stale
number, we cast one fresh bundle while writing this section
(tx [`2iwXTRSUuQzLYK2VuMjn7itEftX62HgyGVRbYLppS8GEaULLPPZnRhsqEjKkAb36YnBUqjp3Cdx8KdQFJqC5GDsH`](https://explorer.solana.com/tx/2iwXTRSUuQzLYK2VuMjn7itEftX62HgyGVRbYLppS8GEaULLPPZnRhsqEjKkAb36YnBUqjp3Cdx8KdQFJqC5GDsH?cluster=devnet))
and re-ran the query: **1 confirmed signature, 1 distinct signer**
(`DumKvuUFpqXXh8CXU1HG7PDhADsfNwNXxKfSEBRzyAJL`) recovered with no knowledge of which of
the bundle's 4 legs was real. On a program with sustained usage and a full-history
indexer (mainnet, or an archival RPC), the same one-call query enumerates every signer
who has ever used the tool.

**Why this isn't closed, and can't be closed alone.** The bounty listing names the
design-space trade-off directly: "either a single global program everyone routes
through, or fresh programs deployed on demand for each use." A fresh program per bundle
would close this channel — but it directly costs the **composability** requirement the
same listing names (`COMPOSABILITY.md`): a fresh, unpredictable program address per use
is not something a caller (a market maker's router, an `account-cooker` agent) can
discover or integrate against without an out-of-band lookup service, and each deployment
carries a real, non-recoverable-until-closed cost — **1.27 SOL** rent-exempt minimum for
a program this size (`solana rent 182408`, this deployment's actual `.so`), paid again
for every fresh instance. We chose one global program deliberately, for that reason, and
are naming the resulting identity leak explicitly rather than leaving it undiscovered.
Closing it without losing composability would need a *stable, discoverable* indirection
layer (a router whose callers are the ones freshly deployed, or an on-chain registry) —
a real, separate design problem, not a small patch, and out of scope for this
submission's remaining time.

## 8. Out of scope (named, not hidden)

- **Same-tx correlation.** All K legs share one transaction and one fee-payer/signer. This
  is inherent to atomic bundling — the anonymity set is *within* the bundle, not across the
  chain. An observer knows these K legs belong together; the defense is that they cannot
  tell *which* leg is real, not that the legs look unrelated.
- **Active / network adversary.** Compromised RPC, key theft, or validator-level ordering
  attacks are outside this model.
- **Off-chain intent leaks.** If the user reveals the real destination elsewhere, no
  on-chain construction helps.

## 9. Summary

| Claim | Status |
|---|---|
| Amount channel closed | Yes — credited reimpl, PR #1's floor |
| Destination-history channel closed | **Yes — measured, +0.598 → ~+0.01 (linear) / +0.09 (nonlinear ensemble, worst case) at K=16, via the deployed select path** |
| Bundle atomic & non-custodial | Yes — program invariants, e2e-proven |
| Funding-graph provenance closed | **No — measured +0.27…+0.51 for durable P2P payees; requires an external crowd** |
| Token-holdings channel closed | **No — measured +0.05…+0.09; requires the same crowd interface (§5.2)** |
| Cross-bundle identity reuse closed | **No — single-bundle closure holds, but a finite pool accumulates a frequency signal over many bundles (+0.58…+0.90 by bundle 25-100, K=8/16); a "spread usage evenly" fix was tried, measured worse, and reverted (§5.3); requires the same crowd interface** |
| Same-tx correlation hidden | No — out of scope by construction |
| Composability (external caller) | Yes — a separate binary depending only on the published SDK, real devnet tx (`COMPOSABILITY.md`) |
| Program-identity leak closed | **No — measured live (1 signer recovered from 1 real tx, zero access to bundle contents); a fresh-program-per-use design would close it but breaks composability (§7)** |

The contribution is the destination-history and composability rows closed, and the
residual rows **measured and scoped honestly** rather than papered over or left as
prose — both point at the same missing piece, the external crowd §5.1 defines the
interface for.
