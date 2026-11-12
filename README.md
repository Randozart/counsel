# Counsel

Basic **pre/post-condition verification** for Rust. The **predicate core is
authored in Briev** — the contract-first language from the
[`briev-lang`](https://github.com/) project — compiled to a native static
library and linked into a Cargo crate; the body prover that discharges static
contracts is part of the Briev compiler itself.

**Who owns what** (measured 2026-11-11, see
[`docs/plans/2026-11-11-briev-share.md`](docs/plans/2026-11-11-briev-share.md)):

| Layer | Language | Where |
|---|---|---|
| Predicates, equality, opcode dispatch | **Briev** (114 lines) | `bridge/counsel.bv` |
| Body prover (paths, bounds, tri-state verdicts) | Rust (1,461 lines) | `briev-lang/src/analysis/verify_bodies.rs`, `symbolic.rs` |
| Rust frontend, compiler discovery, report parsing | Rust (~640 lines) | `counsel-static/src/` |
| Runtime orchestration and sampler loop | Rust (253 lines) | `src/lib.rs` |

The orchestration belongs in Briev too, and will move there once Briev's
cross-ABI callback ABI is fixed (see `BUGS.md` and the plan above).

Positioned as **lighter than Kani**: it enforces obligations at runtime and
cross-verifies a function against a shadow reference. It is not a full
model-checker over the Rust memory model.

> **Status: Phases 0–4 shipped (committed).** The full path —
> `bridge/counsel.bv` → `brievc build --library` → C-ABI bindings → safe Rust
> facade — is working end-to-end. Every Briev export carries and proves its
> own contract; every obligation's truth value is Briev-computed.
>
> **Docs:** [Architecture](docs/ARCHITECTURE.md) (how it works) ·
> [Examples](docs/EXAMPLES.md) (worked end-to-end walks).

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

The other half, `counsel-static`, proves contracts **before** the program runs
by translating a restricted pure-integer Rust subset into Briev `defn`s and
discharging them through `brievc --verify-bodies`.

Full detail: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).
Worked walks: [`docs/EXAMPLES.md`](docs/EXAMPLES.md).

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
| `run_checked` | `(pre, body, post, x) -> Verdict` | same check, C callbacks | **Briev `run_checked`** — ordering + short-circuit |
| `verify_samples` | `(original, shadow, n) -> Verdict` | cross-verification sampler | **Briev `verify_samples`** — the whole loop |
| `verify` | `(original, shadow, n, gen) -> Verdict` | sampler with a Rust input generator | Briev `eq` on each output pair |
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

Primary docs: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) (how the two halves
work, and what is and is not guaranteed) and
[`docs/EXAMPLES.md`](docs/EXAMPLES.md) (worked end-to-end examples with real
prover output).

Background: [`docs/plans/2026-11-11-counsel-briev-verify.md`](docs/plans/2026-11-11-counsel-briev-verify.md)
and the feasibility study + decision record in
[`docs/BRIEV-RUST-VERIFY-PLAN.md`](docs/BRIEV-RUST-VERIFY-PLAN.md).

- **Phase 0** — scaffold spike: build path proven end-to-end. ✅
- **Phase 1 (A1)** — Briev-computed check predicates + Rust `contract` combinator. ✅
- **Phase 2** — cross-verification sampler (`verify`). ✅
- **Phase 3 (A2)** — pure-Briev predicate-as-data (`Op` + `eval_pred`). ✅
- **Phase 4** — static proof: `counsel-static` translates a restricted
  pure-integer Rust subset into Briev `defn`s and discharges the contracts
  through `brievc --verify-bodies`. ✅

## Static verification (Phase 4)

The `counsel-static` crate proves contracts **statically** — no runtime checks,
no sampling. It reuses Briev's own body prover (the `--verify-bodies` gate in
`briev-lang`), so the same machinery that proves Briev functions proves the
translated Rust ones.

```rust
use counsel_static::{check_source, Outcome};

let report = check_source(r#"
    #[pre(a >= 0)]
    #[post(term == a + a)]
    fn double(a: i64) -> i64 { a + a }
"#)?;
assert_eq!(report.verdicts[0].outcome, Outcome::Proven);
# Ok::<(), counsel_static::Error>(())
```

A body that contradicts its contract is caught at check time:

```rust
let report = check_source("#[post(term == 0)]\nfn lies(a: i64) -> i64 { 5 }")?;
assert_eq!(report.verdicts[0].outcome, Outcome::Violated);
```

Three outcomes, honestly distinguished: `Proven` (discharged), `Violated`
(a concrete counterexample exists), and `Unproven` (the prover could not
decide — advisory, never a false pass).

**Subset** (deliberately narrow): `i64` parameters and return, integer
arithmetic, comparisons, `&&`/`||`/`!`, `let`, `if`/`else`, `return`, and a
trailing expression. Anything else is rejected as unsupported rather than
approximated — a wrong translation would be a wrong proof.

Requires a `brievc` built from `briev-lang` `c0ce60dc` or later; set `BRIEVC`
to point at one. Without it, twin tests skip and the translator tests still run.

## Known limitations

**Cross-ABI function-pointer parameters are not runtime-safe in Briev** today:
the C-binding generator advertises a flat `int64_t (*)(int64_t)`, but the LLVM
backend lowers the param to a closure/self convention (`load target from the
slot`, call `(slot, arg)`) — a flat `extern "C" fn` host binding SIGSEGVs.
Counsel therefore transports predicates as **data** (pure Int ABI), never as
callbacks. Full diagnostic + the Briev backend fix path in
[`BUGS.md`](BUGS.md).

**Briev codegen bug found by the valve:** a function whose every path
terminates inside `when` guards (with no trailing `term`) emits a dangling
`ret` on an undefined register. `counsel-static` avoids the shape; minimal
repro and the upstream fix path are in [`BUGS.md`](BUGS.md).

## License

Apache-2.0 WITH LLVM-exception. Compiled output of Briev programs is exempt
from the notice/attribution obligations.