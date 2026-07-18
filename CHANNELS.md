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
| **Destination — history existence** | does the account exist before the bundle slot? | **Closed** (this system's contribution) | open +0.317→+0.598 → defended −0.001→+0.014 (`PROOF.md §2`) |
| **Destination — age / txcount / recency** | profile of prior activity | **Closed** — same warmed-pool matching | all classifiers +0.601 open → +0.014 defended |
| **Atomicity / partial landing** | did only some legs settle? | **Closed** (program invariant I3) | `underfunded_sdk_bundle_reverts_atomically` (`PROOF.md §4`) |
| **Destination — funding provenance** | does the decoy's funding trace to the signer? | **OPEN — measured +0.27…+0.51 for durable P2P payees (most destinations are self-funded plumbing decoys already match); not solvable by self-funded decoys** | §5 below |
| **Timing / co-signing / same-tx correlation** | all legs share one tx and signer | Inherent to the construction; out of scope | §6 |

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

### 5.1 The circular dependency — and the crowd interface

The bounty's three tools form a cycle, and this residual is where they meet:

- **supersonic-tx** closes its dominant leak only with aged, plausibly-funded decoy
  identities → that is **account-cooker**'s job.
- **account-cooker**'s identities are self-funded → the funding graph re-links them →
  they form no anonymity set.
- A crowd of **other people's** activity closes it → that is **mirror-pool**.

The interface a crowd must satisfy to close §5: decoy destinations whose funding
provenance is **not** the signer's — i.e. addresses drawn from a set of mutually
unlinkable, externally-funded participants — presented to `plan_bundle` in place of
self-derived pool members, while preserving I1/I2 (the user must still be able to prove no
custodial hold and recover any misdirected funds). supersonic-tx defines the slot; it does
not implement the crowd.

## 6. Out of scope (named, not hidden)

- **Same-tx correlation.** All K legs share one transaction and one fee-payer/signer. This
  is inherent to atomic bundling — the anonymity set is *within* the bundle, not across the
  chain. An observer knows these K legs belong together; the defense is that they cannot
  tell *which* leg is real, not that the legs look unrelated.
- **Active / network adversary.** Compromised RPC, key theft, or validator-level ordering
  attacks are outside this model.
- **Off-chain intent leaks.** If the user reveals the real destination elsewhere, no
  on-chain construction helps.

## 7. Summary

| Claim | Status |
|---|---|
| Amount channel closed | Yes — credited reimpl, PR #1's floor |
| Destination-history channel closed | **Yes — measured, +0.598 → +0.014 at K=16** |
| Bundle atomic & non-custodial | Yes — program invariants, e2e-proven |
| Funding-graph provenance closed | **No — measured +0.27…+0.51 for durable P2P payees; requires an external crowd** |
| Same-tx correlation hidden | No — out of scope by construction |

The contribution is the middle two rows measured, and the fourth row stated open rather
than papered over.
