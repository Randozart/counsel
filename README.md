# Counsel

Basic **pre/post-condition verification** for Rust.

Runtime checks are plain Rust with no external requirements. The `static`
feature proves contracts **ahead of time** by handing them to the body prover
from [`briev-lang`](https://github.com/), the contract-first language's compiler.

**Briev is only involved if you ask for `--features static`.** The default
feature set compiles no Briev, links no Briev and runs no Briev — `cargo add
counsel` builds offline on any platform.

**Who owns what:**

| Layer | Language | Where |
|---|---|---|
| Runtime checks, equality, opcode dispatch, sampler | Rust | `src/lib.rs` |
| Static-verification frontend (translate, discover, parse reports) | Rust | `src/check/` |
| Body prover (paths, bounds, tri-state verdicts, SMT) | Rust | `briev-lang` |

Positioned as **lighter than Kani**: it enforces obligations at runtime and
cross-verifies a function against a shadow reference. It is not a full
model-checker over the Rust memory model.

> **Status: Phases 0–4 shipped.** The runtime half is pure Rust; the static half
> translates a restricted pure-integer Rust subset into Briev `defn`s and
> discharges them with Briev's own body prover.
>
> **Docs:** [Architecture](docs/ARCHITECTURE.md) (how it works) ·
> [Examples](docs/EXAMPLES.md) (worked end-to-end walks).

## How it works

### Runtime (`default` features) — no Briev

```
src/lib.rs   ← contract, verify, satisfies, checks: ordinary Rust
```

`#[pre]`/`#[post]`/`#[invariant]` are inert markers from the `counsel-macros`
proc-macro crate. There is no build script, no native library and no download.

### Static (`--features static`) — Briev's prover, in process

```
Rust source ──syn──> briev_verify::ast ──> verify_bodies ──> typed verdicts
```

No compiler, no subprocess, no download. The prover is the `briev-verify` crate:
Counsel builds the AST nodes directly and calls it in the same process.

Full detail: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).
Worked walks: [`docs/EXAMPLES.md`](docs/EXAMPLES.md).

## Install

One crate, one dependency:

```toml
counsel = "0.1"                                        # runtime API + attributes
counsel = { version = "0.1", features = ["static"] }   # + static verification
```

| Feature | Gives you | Needs Briev |
|---|---|---|
| `macros` (default) | `#[pre]`/`#[post]`/`#[invariant]` accepted by rustc | no |
| `static` | the `counsel::check` API and the `counsel-check` binary | only the `briev-verify` crate — ordinary Rust, compiled in |

`counsel-macros` is a separate crate only because Rust requires
`proc-macro = true` crates to stand alone; `counsel` re-exports its attributes,
so you never add it yourself.

The attributes are **inert markers** — they enforce nothing by themselves.
Enforcement comes from `counsel-check` (static) or `contract()` (runtime).

`counsel` (the **runtime half**) needs nothing. It is plain Rust: no build
script, no native library, no compiler, no download.

The **static half** (`--features static`) needs nothing either. It adds the
`briev-verify` crate — ordinary Rust compiled into your build — so there is no
binary to find, fetch, checksum or version-pin.

```bash
cargo test --features static
```

## Quick start

```bash
cargo test --features static   # proves every #[pre]/#[post] pair in the suite
```

## What Counsel gives you

| Tool | Signature | What it does | Briev role |
|---|---|---|---|
| `add` | `(a, b) -> i64` | verified integer sum | Briev `add` |
| `positive` / `nonneg` / `in_range` | `(i64) -> bool` | check predicates | Briev `is_positive` / `is_nonneg` / `within` |
| `contract` | `(pre, body, post, x) -> Verdict` | pre→body→post obligation check | each obligation truth from Briev |
| `run_checked` | `(pre, body, post, x) -> Verdict` | same check, C callbacks | **Briev `run_checked`** — ordering + short-circuit |
| `verify_samples` | `(original, shadow, n) -> Verdict` | cross-verification sampler | **Briev `verify_samples`** — the whole loop |
| `verify` | `(original, shadow, n, gen) -> Verdict` | sampler with a Rust input generator — deprecated, prefer verify_samples | Briev `eq` on each output pair |
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
- **Phase 4** — static proof: the `static` feature translates a restricted
  pure-integer Rust subset into Briev `defn`s and discharges the contracts
  through `briev-verify`, in process. ✅

## Static verification (Phase 4)

With `--features static`, Counsel proves contracts **statically** — no runtime
checks, no sampling. It reuses Briev's own body prover through the `briev-verify`
crate, so the same machinery that proves Briev functions proves the Rust ones,
and it runs in process with no compiler involved.

```rust
use counsel::check::{check_source, Outcome};

let report = check_source(r#"
    #[pre(a >= 0)]
    #[post(term == a + a)]
    fn double(a: i64) -> i64 { a + a }
"#)?;
assert_eq!(report.verdicts[0].outcome, Outcome::Proven);
# Ok::<(), counsel::check::Error>(())
```

A body that contradicts its contract is caught at check time:

```rust
let report = check_source("#[post(term == 0)]\nfn lies(a: i64) -> i64 { 5 }")?;
assert_eq!(report.verdicts[0].outcome, Outcome::Violated);
```

Three outcomes, honestly distinguished: `Proven` (discharged), `Violated`
(a concrete counterexample exists), and `Unproven` (the prover could not
decide — advisory, never a false pass).

**Subset** (deliberately narrow): integer parameters and return (i8-i64, u8-u32,
and u64/usize/isize with their range facts), integer arithmetic, comparisons,
`&&`/`||`/`!`, `let`, `if`/`else`, `return`, constant `for` ranges, loops carrying
`#[invariant(...)]`, `max`/`min`/`abs`, and a trailing expression. Anything else
is rejected as unsupported rather than approximated — a wrong translation would be
a wrong proof.

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
`ret` on an undefined register. Counsel avoids the shape; minimal
repro and the upstream fix path are in [`BUGS.md`](BUGS.md).

## License

Apache-2.0 WITH LLVM-exception. Compiled output of Briev programs is exempt
from the notice/attribution obligations.