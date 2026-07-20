# COMPOSABILITY — an independent tool casting through this router, on real devnet

> The bounty's brief asks that "other tools, including account-cooker, can cast through
> [supersonic-tx]." `README.md` documents the interface and a doctest that CI runs. This
> file is the same claim exercised end to end, by code that only depends on the published
> `supersonic-sdk` crate, settling a real transaction on devnet.

## What this is

`composability-demo/` is a standalone Rust binary, its own `Cargo.toml`, depending on
`supersonic-sdk` the same way any external project would (a real integrator would pin a
git rev instead of a path — the path here exists only because the crate under test lives
in this same repo). It never imports the CLI, never touches program source, never calls a
private helper. Everything it does is: build a warmed pool, call `plan_bundle`, get back a
plain `Instruction` from `build_instruction`, sign, send.

```
$ cargo run -p composability-demo -- \
      --to FGgTTG9sBexdHqt2TAVQikRFouhRs2eZbvNfyoYiqzQr \
      --amount 1000000 --k 4 \
      --keypair id.json --rpc https://api.devnet.solana.com --broadcast

composability-demo: bundle K=4, 8000000 lamports moved, via supersonic-sdk only
  leg 0:        1000000 lamports -> 4GHqrqMwUDSUKsh6P5NujQPnQZk1RpAVWpPUTrQe15ix
  leg 1:        2000000 lamports -> HUNxL73SAstNFKmg7wRMy5tBF1CcZvSHN1cbeBeRqAhr
  leg 2:        4000000 lamports -> Giz6yoH1cek6MbCwX7E5ZXvuGJyiAfGebNVQ6EW4Xpsq
  leg 3:        1000000 lamports -> FGgTTG9sBexdHqt2TAVQikRFouhRs2eZbvNfyoYiqzQr

BROADCAST — signature: KAP8cfKvRyqV8f5PXe58LrFnJSCbrqWhHbqiJca7fRtfaYUcHqyjr2wYVuTvWnAuethT565iNYqBg7FFdRYxSNw
```

Live on devnet:
[program `D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be`](https://explorer.solana.com/address/D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be?cluster=devnet) ·
[bundle tx, status Ok](https://explorer.solana.com/tx/KAP8cfKvRyqV8f5PXe58LrFnJSCbrqWhHbqiJca7fRtfaYUcHqyjr2wYVuTvWnAuethT565iNYqBg7FFdRYxSNw?cluster=devnet) —
all 4 legs settled atomically; the real leg (`FGgT…`) received its `0.001 SOL` alongside
three decoys.

## Why `account-cooker` isn't the caller here

The pitch is that a cooker's warmed identities are exactly what a pool member needs to be,
so a cooker could *be* the caller. As of this writing `solanabr/account-cooker` (all three
open PRs) ships `[[bin]]`-only scaffolding — no library target another crate can depend
on, and its agents run indefinitely against real protocols rather than returning a
usable identity synchronously. There is nothing to import yet. `composability-demo`'s
`demo_pool()` plays that missing role directly — the same construction as the SDK's own
published doctest — so this proof is honest about what it shows and doesn't:

- **It shows**: the integration surface is real. Any Rust binary, declared as an ordinary
  path/git dependency, can plan and submit a channel-complete bundle through the deployed
  program without touching this repo's internals. That is the composability requirement,
  met by code that runs, not by a diagram.
- **It does not show**: that a *specific* third-party account-cooker persona is itself
  warmed enough to survive the destination-history channel (`CHANNELS.md §3.2`) — that
  claim needs a real cooker with a lib target and months of aged accounts, which does not
  exist in this ecosystem yet. `demo_pool()`'s members are synthetic, exactly as declared.

## Compare: PR #1's composability proof

`Jmkoygg`'s PR #1 has a sibling composability PR from `xinaids` (account-cooker #1 →
supersonic-tx #3) proving the same requirement in the other direction: an account-cooker
persona casting through *his* router. That proof and this one are the same exercise,
pointed at different routers; neither depends on the other, and this file names it instead
of leaving the comparison implicit.
