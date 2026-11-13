# Counsel — Architecture

How Counsel works, as shipped. Two independent halves, only one of which
involves Briev:

| Half | Crate / feature | When | Question it answers |
|---|---|---|---|
| **Runtime checks** | `counsel` | while the program runs | "did this obligation hold for *this* input?" |
| **Static proof** | `counsel` + `--features static` | before compiling / in CI | "does this body hold for *every* input?" |

Both are deliberately narrow and honest: an unproven obligation is never
reported as a pass.

---

## 1. The two halves at a glance

```
                          src/lib.rs  (runtime checks, pure Rust)
                                 │
                    ┌────────────┴────────────┐
             runtime callers            (Phase 4 valve)
                                              │
                    counsel::check: Rust source text
                                              │  syn parse + subset check
                                              ▼
              briev_verify::ast (built directly, in process)
                                              │  verify_bodies
                                              ▼
                          Proven / Unproven / Violated
```

The runtime half is **ordinary Rust**: no build script, no native library, no
download. The static half builds the prover's AST nodes directly and calls
Briev's prover in the same process — no compiler, no subprocess, no pinned
toolchain. Neither half re-implements a prover.

### Who owns what

| Layer | Language | Where |
|---|---|---|
| Runtime checks, equality, opcode dispatch, sampler | Rust | `src/lib.rs` |
| Static frontend: translate, discover, report parsing | Rust | `src/check/` |
| Body prover (paths, bounds, tri-state verdicts, SMT) | Rust | `briev-lang` |

The runtime half used to be Briev-authored (`bridge/counsel.bv`, 167 lines),
compiled by `build.rs` into a static library linked into the crate. It was a
binding layer over about seven integer predicates, and it forced every consumer
to obtain a Briev compiler before their build could start, so it was ported to
Rust and the whole build path deleted — see
[`docs/plans/2026-11-14-briev-dependency-assessment.md`](plans/2026-11-14-briev-dependency-assessment.md).

---

## 2. Runtime half — `counsel`

### 2.1 Build pipeline

There is none. The runtime half is plain Rust: `cargo build` compiles
`src/lib.rs` and `counsel-macros`, and nothing else. The crate has no build
script, so the default feature set resolves no compiler, produces no native
archive and links nothing.

### 2.2 The runtime checks

Six small integer predicates, implemented directly in `src/lib.rs`. They were
previously Briev `export defn`s compiled by `build.rs`; porting them removed the
build-time compiler requirement.

| Function | Returns | Used for |
|---|---|---|
| `add(a, b)` | `a + b` (wrapping) | plumbing validation |
| `in_range(x, lo, hi)` | `lo ≤ x ≤ hi` | range obligation |
| `positive(x)` | `x > 0` | sign obligation |
| `nonneg(x)` | `x ≥ 0` | sign obligation |
| `eq(a, b)` | `a == b` | sampler output comparison |
| `satisfies(op, x)` | predicate truth | dispatch: `GT`, `GE`, `LT`, `LE`, `In` |

`run_checked` and `verify_samples` keep their `extern "C"` callback signatures
so the public API is unchanged, but they are ordinary Rust functions now.
Nothing crosses an FFI boundary, which is why the cross-ABI callback defect
recorded in [`../BUGS.md`](../BUGS.md) stopped mattering.

### 2.3 Safe facade (`src/lib.rs`)

| API | Shape | Truth computed by |
|---|---|---|
| `Verdict` | `Pass` \| `Fail(String)` | — |
| `add(a, b)` | `i64` | Briev |
| `in_range(x, lo, hi)`, `positive(x)`, `nonneg(x)` | `bool` | Briev |
| `within`, `is_positive`, `is_nonneg` | `Verdict` | Briev |
| `contract(pre, body, post, x)` | `Verdict` | orchestration in Rust, predicates as given |
| `Op` + `satisfies(op, x)` / `check_op(op, x)` | `bool` / `Verdict` | Briev (`eval_pred`) |
| `eq(a, b)` | `bool` | Briev |
| `verify(original, shadow, n, gen)` | `Verdict` | Briev `eq` per sample |

**Division of labour.** Rust owns ordering and value plumbing; Briev owns truth
values. `contract` runs pre → body → post, short-circuiting:

```rust
contract(positive, |x| x * 2, |_, ret| nonneg(ret), 5)   // Pass
contract(positive, |x| x * 2, |_, ret| nonneg(ret), -3)  // Fail("precondition failed")
```

**Caveat, stated plainly:** `contract`'s predicates are ordinary Rust closures.
If you write `|x| x > 0` inline, *Rust* decides and Briev is not consulted. Pass
Briev's own checks (`positive`, `nonneg`, `in_range`) — or the `Op` form — to
have Briev compute the verdict. There is no attribute-injection macro yet.

**Sampler.** `verify` ports Briev's `cross_verify_samples` / `:=` idea: sample
`i`, feed `gen(i)` to both functions, compare outputs through Briev `eq`, report
the first divergence with input, expected and actual.

---

## 3. Static half — `counsel::check` (`--features static`)

### 3.1 Pipeline

```
check_source(&str)                     (or check_file(&Path))
  │
  ├─ syn::parse_file                     parse Rust
  ├─ build::definition_for               per fn: parameters + domain facts,
  │    ├─ loop invariants → _init/_preserve/_exit
  │    └─ #[case(..)] instantiates at concrete inputs
  ├─ verify_bodies(&items)               run the prover in process
  └─ classify per definition             proven / unproven / violated / case
```

`Report` exposes `all_proven()`, `violated()` and `unproven()`.

### 3.2 Translation rules

| Rust | Briev |
|---|---|
| `#[pre(a >= 0)]` | `[a >= 0]` |
| `#[post(term == a + a)]` | `[term == a + a]` |
| `fn double(a: i64) -> i64 { … }` | `defn double(a: Int) [..][..] -> Int { … };` |
| `let m = n + 1;` | `let m: Int = n + 1;` |
| `return e;` | `term e;` |
| trailing `e` | `term e;` |
| `if c { A }` | `when c { A };` |
| `if c { A } else { B }`, both arms terminate | `when c { A };` then `B` **unguarded** |
| `if c { A } else { B }`, possible fall-through | `when c { A };` then `when <inverted c> { B };` |

The unguarded-else form is exact: reaching `B` implies `c` failed, because `A`
always terminated. It is chosen when `block_terminates` / `expr_terminates` /
`if_terminates` prove termination on both arms; otherwise the inverted guard is
kept, which also hands the prover a real numeric fact (`a <= 5`).

`term` is the postcondition's name for the returned value; `false`/`true` are
passed through unchanged.

### 3.3 The subset

Accepted: integer parameters and return (any width, with domain facts), integer literals, identifiers, unary
`-`/`!`, arithmetic `+ - * / %`, comparisons, `&&`/`||`, `let`, `if`/`else`,
`return`, a trailing expression, bare-name assignment `x = expr`, calls to
precondition-free functions (inlined before translation), `for` loops over
constant integer ranges (unrolled, at most 64 iterations), and
`#[pre]`/`#[post]`/`#[invariant]`.

A `for` loop carrying `#[invariant(...)]` is not unrolled. It becomes three
loop-free defns — `<fn>_init`, `<fn>_preserve`, `<fn>_exit` — whose conjunction
is the loop's correctness: the invariant holds on entry, the body preserves it,
and the exit state meets the postcondition. The same solver discharges all
three. A loop is reported proved only when all three are.

Rejected with `Error::Unsupported` (never approximated): method calls, field
access, indexing, slicing, standalone range expressions, `match`, pattern
`let`s, `self` methods, non-integer types, generics with arguments, `async`,
`for` loops whose bounds are neither constant nor covered by an invariant, and
calls to callees that are impure or carry a precondition.

Rationale: a wrong translation would be a wrong proof.

### 3.4 Discharge and classification

`verify_bodies` runs in process and returns a `BodyReport` with three lists:
what it proved, what it could not decide, and what it refuted.

| Report list | Outcome |
|---|---|
| `violated` | `Violated` |
| `unproven` | `Unproven` |
| `proven` | `Proven`, or `ProvenForCase` for a `<fn>__caseN` instantiation |
| none of them | `Unproven` — the prover never examined it |

The last row is the point. A definition named nowhere was skipped, and silence is
never a proof, so classification never infers `Proven` from absence. There is no
prose to parse: the verdicts are a typed structure, so a change in wording cannot
silently turn a violation into a proof.

---

## 4. Under the hood: Briev's body prover

Lives in the `briev-verify` crate (`src/verify.rs`), extracted from the compiler
so it can be used without it; the compiler consumes the same code through a
re-export, so there is one implementation.

- **Path walk:** a worklist over statement blocks; each `when` guard forks into
  taken (guard added as a fact) and fall-through (negation added).
- **Result binding:** `term`, `endprogram` values and defn-body tails are bound
  as the declaration's result, so postconditions can speak about it.
- **Facts → bounds:** comparison facts (`x > 5`, `x <= n`, conjunctions, and
  negated forms) tighten per-identifier integer intervals.
- **Tri-state verdict per path:** `Holds` / `Violated` / `Undecided`, composed
  with Kleene `and`/`or`/`not`; structural equality proves `x + 1 == x + 1`
  without a numeric value.
- **Soundness boundary:** `Unknown` never equals anything; bodies mentioning
  calls, fields, indexes, slices, loops or other unmodelled constructs are
  **skipped** — never proven, never falsely rejected.
- **Aggregation:** `BodyReport { proven, unproven, violated }`. `compile.rs`
  turns `violated` into a hard error and `unproven` into an advisory warning.
- **Solver escalation (C4):** an `Undecided` path is handed to Z3 as
  `facts ∧ ¬post`. `Unsat` ⇒ proven; `Sat` ⇒ refuted only when the result is
  fully determined (no free identifier survives substitution); `Unknown`, or no
  solver installed, ⇒ `Unproven` as before. A parameter-dependent
  counterexample stays advisory: the stdlib's optimistic `[true][term >= 0]`
  contracts are refutable for some inputs and must not fail a build.

The static half inherits exactly this behaviour, which is why `Unproven` never
counts as a pass.

---

## 5. Guarantees

| Guarantee | Status |
|---|---|
| A `Violated` verdict implies a concrete counterexample path | Yes |
| `Unproven` is never treated as success | Yes — advisory only |
| Bodies outside the subset are skipped, not guessed | Yes |
| Runtime checks are cheap, pure `Int` calls into native code | Yes |
| Static proof needs no runtime execution | Yes |
| Full Rust support | **No** — integer subset only; any integer width (i8-i64, u8-u32, u64/usize/isize), and calls resolve when the callee is pure and precondition-free |
| Deciding every true contract | **No** — bounds + Z3 when installed; parameter-dependent goals stay advisory |
| Works without a Briev compiler | **Yes** — the prover is a Rust library, no binary and no download |

---

## 6. Where to look

| Path | Contents |
|---|---|
| `src/lib.rs` | runtime checks, sampler, predicate dispatch (pure Rust) |
| `src/check/build.rs` | Rust syntax → prover AST (loops, invariants, cases) |
| `src/check/inprocess.rs` | `verify_bodies` → `Report` |
| `src/check/mod.rs` | `Outcome`, `Report`, `check_source`, `check_file` |
| `tests/smoke.rs`, `tests/valve.rs` | executable specification |
| `BUGS.md` | the cross-ABI callback blocker and the Briev codegen bug |
| `docs/EXAMPLES.md` | worked end-to-end walkthroughs |

## 7. Reproduce

```bash
# Runtime half: plain Rust, no compiler involved
cargo test -p counsel

# Static half: drives the real prover in process (no compiler involved)
cargo test --features static

# Whole workspace (counsel + the attribute companion)
cargo test --workspace --all-features
```
