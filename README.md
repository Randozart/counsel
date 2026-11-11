# Counsel

Basic **pre/post-condition verification** for Rust, with the verification
machinery **authored in Briev** — the contract-first language from the
[`briev-lang`](https://github.com/) project — compiled to a native static
library and linked into a Cargo crate.

Positioned as **lighter than Kani**: it enforces obligations at runtime and
cross-verifies a function against a shadow reference. It is not a full
model-checker over the Rust memory model.

> **Status: Phases 0–3 shipped (committed).** The full path —
> `bridge/counsel.bv` → `brievc build --library` → C-ABI bindings → safe Rust
> facade — is working end-to-end. Every Briev export carries and proves its
> own contract; every obligation's truth value is Briev-computed.

## How it works

```
bridge/counsel.bv   ← verification machinery, authored in Briev
        │  (brievc build --library → libcounsel.a)   [build.rs]
        ▼
src/bindings.rs     ← checked-in C-ABI bindings (regenerated when exports change)
        ▼
src/lib.rs          ← safe Rust facade: contract, verify, satisfies, checks
```

`build.rs` shells to `brievc` (selectable via the `BRIEVC` env var, default
`brievc` on PATH), compiles the `.bv` machinery to a static library, and links
it into the crate.

## Quick start

```bash
# Build linked library + machine tests
BRIEVC=/path/to/brievc cargo test
```

or, if `brievc` is already on your PATH:

```bash
cargo test
```

## What Counsel gives you

| Tool | Signature | What it does | Briev role |
|---|---|---|---|
| `add` | `(a, b) -> i64` | verified integer sum | Briev `add` |
| `positive` / `nonneg` / `in_range` | `(i64) -> bool` | check predicates | Briev `is_positive` / `is_nonneg` / `within` |
| `contract` | `(pre, body, post, x) -> Verdict` | pre→body→post obligation check | each obligation truth from Briev |
| `verify` | `(original, shadow, n, gen) -> Verdict` | cross-verification sampler | Briev `eq` on each output pair |
| `satisfies` / `check_op` | `(Op, i64)` | predicate-as-data, evaluated in Briev | Briev `eval_pred` |

## Examples

```rust
use counsel::{Verdict, Op, contract, nonneg, positive, satisfies, verify, within};

// A1: Briev-computed obligations composed in Rust.
assert_eq!(contract(positive, |x| x * 2, |_, ret| nonneg(ret), 5), Verdict::Pass);
assert_eq!(contract(positive, |x| x * 2, |_, ret| nonneg(ret), -3),
           Verdict::Fail("precondition failed".into()));

// A2: predicate-as-data; Briev eval_pred computes the truth value.
assert!(satisfies(Op::In { lo: 0, hi: 10 }, 5));
assert_eq!(within(5, 0, 10), Verdict::Pass);

// Sampler: original and shadow must agree over every sample.
assert_eq!(verify(|x| x * 2, |x| x * 2, 20, |i| i as i64), Verdict::Pass);
```

## Phase plan

See [`docs/plans/2026-11-11-counsel-briev-verify.md`](docs/plans/2026-11-11-counsel-briev-verify.md)
and the feasibility study + decision record in
[`BRIEV-RUST-VERIFY-PLAN.md`](../BRIEV-RUST-VERIFY-PLAN.md).

- **Phase 0** — scaffold spike: build path proven end-to-end. ✅
- **Phase 1 (A1)** — Briev-computed check predicates + Rust `contract` combinator. ✅
- **Phase 2** — cross-verification sampler (`verify`). ✅
- **Phase 3 (A2)** — pure-Briev predicate-as-data (`Op` + `eval_pred`). ✅
- **Phase 4** — stretch slice of static proof (research; optional).

## Known limitation

Cross-ABI **function-pointer parameters are not runtime-safe in Briev** today:
the C-binding generator advertises a flat `int64_t (*)(int64_t)`, but the LLVM
backend lowers the param to a closure/self convention (`load target from the
slot`, call `(slot, arg)`) — a flat `extern "C" fn` host binding SIGSEGVs.
Counsel therefore transports predicates as **data** (pure Int ABI), never as
callbacks. Full diagnostic + the Briev backend fix path in
[`BUGS.md`](BUGS.md).

## License

Apache-2.0 WITH LLVM-exception. Compiled output of Briev programs is exempt
from the notice/attribution obligations.