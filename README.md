# supersonic-tx

[![ci](https://github.com/gustavo-f0ntz/supersonic-tx/actions/workflows/ci.yml/badge.svg)](https://github.com/gustavo-f0ntz/supersonic-tx/actions/workflows/ci.yml)

**A channel-complete decoy system for Solana transfers.** Hide which of K legs in an
atomic bundle is the real payment — by matching the real leg in *every* observable
channel, not just the amount.

Superteam Brasil "Privacy-Through-Noise" bounty submission. Standalone, Rust-only,
non-custodial.

## The one-paragraph pitch

A bundle's legs expose two things: **amount** and **destination**. Prior work
([@Jmkoygg's PR #1](https://github.com/solanabr/supersonic-tx/pull/1)) closes the amount
channel well but leaves destinations as fresh keys with zero history — while **63.4% of
real mainnet transfer destinations already have on-chain history** (n=1181, measured).
One `getSignaturesForAddress` per leg turns that gap into a **+0.60 advantage at K=16 —
roughly 50× the amount-channel advantage**. This system closes that channel with a
pre-warmed decoy pool whose history profile is drawn from the same distribution as real
payees, fails closed when it can't match, and **measures** the residual it cannot close (the
funding graph). It is the 2017→2018 Monero decoy-selection lesson, ported to Solana.

## Read in this order

1. **[DESIGN.md](DESIGN.md)** — the thesis, the measurement, the defense, and where it stops.
2. **[PROOF.md](PROOF.md)** — reproducible evidence: every number, one command each.
3. **[CHANNELS.md](CHANNELS.md)** — channel-by-channel status, closed and open, and the crowd interface the open residual needs.

## Layout

```
programs/supersonic-tx   atomic K-leg transfer router (non-custodial, atomic, recoverable)
supersonic-sdk           bundle planner: amount layer (exchangeable, credited to PR #1)
                         + destination layer (matched pre-warmed pool)  ← the contribution
cli  (supersonic)        warm / plan / inspect / recover (offline) + send (simulates
                         by default; --broadcast to submit to an RPC)
dest-harness             destination-channel adversary + advantage measurement
e2e                      SDK-planned bundle settles on the real program in LiteSVM
```

## Quickstart

```bash
# 1. build the program artifact the e2e tests load by path (needs the Solana toolchain)
cargo build-sbf --manifest-path programs/supersonic-tx/Cargo.toml

# 2. everything, one command — 48 tests across all five crates
cargo test --workspace
#   (step 1 is required first: the 8 e2e tests load the .so and fail loudly,
#    with a "run cargo build-sbf" message, if it isn't built)

# reproduce the headline measurement (real mainnet data, in-repo)
cd dest-harness
cargo run -p supersonic-dest-harness --bin dest-advantage -- \
    --study data/dest_study.jsonl --n 8000 --seed 1

# the CLI (warm/plan/inspect/recover are offline; send simulates unless --broadcast)
cargo run -p supersonic-cli -- --help
```

## What it does not claim

The defense closes the destination-history channel to a measured floor (+0.014 at K=16,
the same order as PR #1's amount-channel floor). It does **not** close funding provenance:
a decoy funded by the signer re-links one hop out, so a third-party-funded real payee still
stands out. We measured it — for durable P2P payees, **86.5% are third-party-funded, a
residual of +0.27–0.51** (`PROOF.md §6`) that no self-funded decoy scheme can close; only
decoys funded by unlinkable third parties (a crowd) can. (Most *raw* transfer destinations,
though, turn out to be self-funded transient token accounts a decoy already matches — the
residual is a property of who you actually pay, not the transfer population at large.) See
[CHANNELS.md §5](CHANNELS.md).

**Deployed on devnet** — [program `D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be`](https://explorer.solana.com/address/D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be?cluster=devnet),
with a real CLI-planned bundle settled on chain ([tx](https://explorer.solana.com/tx/5zd9D1V51Grjjm9tB38jimgdAHwAb3n5s4gqgwYUQgx5FMoygJpKKkEhVciZCNGS56NVxZJKNDsq65ADw8GtojjR?cluster=devnet), status Ok). See [PROOF.md §4.1](PROOF.md).

## Credit

The exchangeable amount construction is [@Jmkoygg](https://github.com/solanabr/supersonic-tx/pull/1)'s;
we reimplement it with attribution as our amount layer. The destination layer, harness,
program, CLI, and the mainnet payee-profile dataset are ours.
