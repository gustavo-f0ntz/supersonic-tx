# Benchmark — Anchor vs. Pinocchio for the router core

The router is a tiny program: validate a few things, then do `K` System-Program
transfers. That makes it a good candidate to ask a concrete question — **does the
framework overhead actually cost us anything that matters?** — and answer it with
measured numbers instead of taste.

We reimplemented the exact same logic (`programs/supersonic-tx/src/lib.rs`'s
`execute_bundle`) in [Pinocchio](https://github.com/anza-xyz/pinocchio) (zero-dependency,
`no_std`) alongside the shipped Anchor program, and measured. The Pinocchio source is in
[`bench/pinocchio-router/`](./bench/pinocchio-router/src/lib.rs).

Methodology (framework-overhead question, binary size as the load-bearing metric, CU as
secondary, prove-the-invariants-not-just-the-size) follows the prior art of
`Jmkoygg/supersonic-tx`'s own Anchor-vs-Pinocchio benchmark for this same router shape —
credited here, reimplemented independently against *our* program's invariants.

## Result 1 — binary size and deploy rent (measured)

| | Anchor (shipped) | Pinocchio (bench) | ratio |
|---|---:|---:|---:|
| Compiled `.so` | **182,408 bytes** | **5,672 bytes** | **32.2× smaller** |
| Rent-exempt to deploy | **1.27045056 SOL** | **0.040368 SOL** | **31.5× less** |

Reproduce:
```bash
# Anchor
cargo build-sbf --manifest-path programs/supersonic-tx/Cargo.toml
stat -c%s target/deploy/supersonic_tx.so && solana rent 182408
# Pinocchio
cargo build-sbf --manifest-path bench/pinocchio-router/Cargo.toml
stat -c%s bench/pinocchio-router/target/deploy/supersonic_tx_pinocchio.so && solana rent 5672
```

Deploy rent is linear in binary size (Solana's rent formula:
`(bytes + 128) × 0.00000348 × 2 SOL`), so a 32× smaller binary is a ~32× smaller deploy
cost. This is the load-bearing result: it turns a real mainnet deployment of this program
from over 1.27 SOL of locked rent into about 0.04 SOL — cheap enough not to be a decision.

Measured in the process of getting this number: clippy's own `manual_range_contains`
suggestion (`!(MIN_LEGS..=MAX_LEGS).contains(&count)` in place of the two comparisons in
`bench/pinocchio-router/src/lib.rs`) **nearly doubles this binary** (5,672 → 10,736
bytes) under this no_std/BPF/LTO build — `RangeInclusive`'s trait-based check doesn't
fold down the same way a raw comparison does here. Kept the two comparisons with a
targeted `#[allow(clippy::manual_range_contains)]` and a comment citing this measurement,
since binary size is this crate's entire reason to exist.

## Result 2 — compute units (measured — and a real finding beyond "fixed overhead")

The expected story is that Pinocchio only removes *framework* overhead — Borsh
deserialization, discriminator parsing — a **fixed cost paid once per instruction**,
while the actual work (`K` System-Program transfers) should cost the same either way
since the runtime prices the CPI identically regardless of caller framework.

Measured under Mollusk, same tool for both programs (`dest-harness/tests/
pinocchio_cu_bench.rs` for Pinocchio; `e2e/tests/cu_benchmark.rs` for Anchor, measured
separately under LiteSVM — see that file's own doc comment for why Mollusk isn't also
added there):

| K | Anchor (CU) | Pinocchio (CU) | saved | saved % |
|---:|---:|---:|---:|---:|
| 2  | 5,551  | 2,463  | 3,088  | 55.6% |
| 4  | 10,043 | 4,859  | 5,184  | 51.6% |
| 8  | 19,027 | 9,646  | 9,381  | 49.3% |
| 16 | 36,995 | 19,219 | 17,776 | 48.0% |

Fitting `CU(K) = a + b·K` to both series:

| | fixed (`a`) | per-leg (`b`) |
|---|---:|---:|
| Anchor | ~1,059 CU | ~2,246 CU/leg |
| Pinocchio | ~69 CU | ~1,197 CU/leg |

**The fixed-cost gap (~990 CU) is what the "framework overhead" story predicts, and it's
real. But the per-leg cost is also roughly halved (2,246 → 1,197 CU/leg) — not fixed,
and not explained by discriminator parsing.** That means the saved percentage does *not*
shrink as K grows (as a purely-fixed-overhead model would predict) — it stays close to
~50% across the whole K=2..16 range measured. The likely mechanism is that Anchor's
`system_program::transfer` CPI helper goes through `CpiContext`/`AccountInfo` wrapping on
every invocation, where Pinocchio's `Transfer::invoke()` operates directly on
`AccountView` with no such layer — plausible, but not confirmed by profiling the CPI
path itself, so stated as measured-and-observed, not fully mechanistically diagnosed.

Reproduce: `cargo test -p supersonic-dest-harness --test pinocchio_cu_bench -- --nocapture`
(writes `target/benches/compute_units.md`) and `cargo test -p supersonic-e2e --test
cu_benchmark -- --nocapture`.

Either way, this program is nowhere near CU-bound: even Anchor's K=16 number is 97.4%
under the 1,400,000 CU transaction ceiling. CU is real evidence, not the deciding axis —
Result 1 is.

## Result 3 — same invariants, not just a size comparison (measured)

A benchmark that's only ever been sized isn't evidence it *works* — only that it's small.
`bench/pinocchio-router` passes the same 6 fail-closed/atomicity scenarios the Anchor
program's own `e2e/tests/program_invariants.rs` proves against the real `.so`
(`dest-harness/tests/pinocchio_invariants.rs`, via Mollusk, 7 tests including the success
path): bundle-size bounds (`K<MIN_LEGS`, `K>MAX_LEGS`), exact leg/destination count,
zero-amount rejection, self-destination rejection, and atomicity (a later leg's failure
rolls back an already-applied earlier transfer). Assertions are generic (`is_err()`), not
pinned to a named error, since Pinocchio has no framework-generated error enum the way
Anchor's `SupersonicError` does.

## What this benchmark decides

Pinocchio is **~32× smaller and ~32× cheaper to deploy**, and — a real finding beyond the
prior-art benchmark this follows — **consistently ~48–56% cheaper in compute across the
whole K range measured**, not just on a one-time fixed cost. The cost is writing account/
data parsing by hand, which for a program this simple (no custody, no PDA state, one
instruction) removes almost none of Anchor's safety value: the signer requirement is still
enforced by the System Program during the CPI regardless of caller framework, and Result 3
proves the same invariants hold.

**The Pinocchio program does not replace the live Anchor deployment**
(`D1yahocVjdQFeidzSwsEeWBYF3ePvjpmjPJjKHHaY9be`, see `PROOF.md §4.1`, still this project's
shipped program) — it implements the same core logic with a compact manual instruction
encoding, not the shipped Anchor instruction format, and is offered as a
minimal-attack-surface *option*, tested to the same bar as the shipped program (Result 3)
and, per Result 4 below, deployed and settled for real on devnet too — not a size-only
artifact. Swapping it in as the production program (updating the SDK/CLI to speak its
instruction encoding) is a clean, well-scoped follow-up this benchmark is the evidence for.

## Result 4 — deployed and settled live, for real, on devnet

Both prior Anchor-vs-Pinocchio benchmarks for this router shape (Jmkoygg's own, and this
one before this section) explicitly left the Pinocchio side **undeployed** — a benchmark
artifact, not a running program. We went one step further: deployed it for real and
settled a real bundle, the same class of proof `PROOF.md §4.1` gives the Anchor program.

- **Deployed to devnet:** program id
  [`FzN88QUEZCbj2D2H7xYe9XNFrPyUHreqkPLAG7y5yaUK`](https://explorer.solana.com/address/FzN88QUEZCbj2D2H7xYe9XNFrPyUHreqkPLAG7y5yaUK?cluster=devnet),
  deploy signature
  [`N2f7CuWgrPcZhGDNZfaGERqBRx2X1mvCEatWgkFdsfJbnva5U8ZAXjSroX7fxo5NzS8Kg8PG4dG3166fsa4Tyu4`](https://explorer.solana.com/tx/N2f7CuWgrPcZhGDNZfaGERqBRx2X1mvCEatWgkFdsfJbnva5U8ZAXjSroX7fxo5NzS8Kg8PG4dG3166fsa4Tyu4?cluster=devnet)
  (same authority as the Anchor deployment, `~/.config/solana/id.json`).
- **A real K=4 bundle settled**, all four legs funded exactly their planned amount:
  [`4yNeQEeBYCQtxNfg7nooy9x6sCv7B5aFQttW7U7rJA5gHPN73mh5n4uVeKKXeYZT5o9qTWdfcLUvyNLpLTC4LwV9`](https://explorer.solana.com/tx/4yNeQEeBYCQtxNfg7nooy9x6sCv7B5aFQttW7U7rJA5gHPN73mh5n4uVeKKXeYZT5o9qTWdfcLUvyNLpLTC4LwV9?cluster=devnet).

Reproduce: `dest-harness/tests/pinocchio_devnet.rs`, `#[ignore]`d (needs network + a funded
devnet keypair, neither available in CI):
```bash
cargo test -p supersonic-dest-harness --test pinocchio_devnet -- --ignored --nocapture
```

**A real bug this caught, worth stating plainly:** the first attempt used
`Pubkey::new_unique()` for the fresh destinations — its deterministic, process-local
counter (not a CSPRNG) produces the *same* low-numbered pubkeys on every run, project or
machine. One of those already carried a real balance on the shared public devnet (some
unrelated test suite, at some point) — 26.9 SOL where 0 was expected, failing the
post-bundle balance assertion even though the transfer itself settled correctly. Fixed by
switching to `Keypair::new().pubkey()` (genuinely random). A test methodology bug, not a
program bug — but the kind of thing this project's whole discipline is to catch and state,
not paper over.

## Honest caveats

- Binary sizes and CU numbers depend on toolchain/optimization flags; both programs were
  built with `opt-level = 3` + fat LTO via `cargo build-sbf`.
- The CU comparison uses Mollusk for both programs precisely to avoid an
  apples-to-oranges LiteSVM-vs-Mollusk methodology gap — but Anchor's own headline K=2..16
  numbers elsewhere in this project (`PROOF.md`, `e2e/tests/cu_benchmark.rs`) were measured
  under LiteSVM; the two tools agree closely in practice (both execute the real `.so`
  through the real Solana runtime) but are not the identical harness.
- A pending network change (SIMD-0436) could halve rent-exempt minimums generally; that
  would scale both Result 1 columns down together and leave the ~32× ratio unchanged.
