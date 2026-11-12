# Counsel — Architecture

How Counsel works, as shipped. Two independent halves share one Briev core:

| Half | Crate | When | Question it answers |
|---|---|---|---|
| **Runtime checks** | `counsel` | while the program runs | "did this obligation hold for *this* input?" |
| **Static proof** | `counsel-static` | before compiling / in CI | "does this body hold for *every* input?" |

Both are deliberately narrow and honest: an unproven obligation is never
reported as a pass.

---

## 1. The two halves at a glance

```
                        bridge/counsel.bv   (Briev predicate core)
                                 │
              brievc build --library  →  libcounsel.a
                                 │            ▲
                     build.rs ───┘            │ link
                                 │            │
                          src/bindings.rs  (C ABI, checked in)
                                 │
                          src/lib.rs  (safe facade)
                                 │
                    ┌────────────┴────────────┐
             runtime callers            (Phase 4 valve)
                                              │
                    counsel-static: Rust source text
                                              │  syn parse + subset check
                                              ▼
                     Briev `defn` source (generated)
                                              │  brievc --verify-bodies
                                              ▼
                          Proven / Unproven / Violated
```

The runtime half **calls** Briev-compiled code; the static half **generates**
Briev code and asks Briev's own prover about it. Neither re-implements a prover
in Rust.

### Who owns what (measured 2026-11-11)

| Layer | Language | Lines | Where |
|---|---|---:|---|
| Predicates, equality, opcode dispatch | **Briev** | 167 | `bridge/counsel.bv` |
| Obligation orchestration (`run_checked`) + sampler loop (`verify_samples`) | **Briev** | same file | `bridge/counsel.bv` |
| Body prover (paths, bounds, tri-state verdicts) | Rust | 1,461 | `briev-lang/src/analysis/{verify_bodies,symbolic}.rs` |
| Rust frontend, compiler discovery, report parsing | Rust | ~640 | `counsel-static/src/` |
| Facade: callback marshalling + verdict mapping | Rust | 308 | `src/lib.rs` |

The orchestration and the sampling loop moved into Briev once the cross-ABI
callback ABI was fixed — `briev-lang` 43519b01 (a host's flat C callback) and
0ea0fe6e (forwarding a callback parameter, which the sampler's recursion
needs). The Rust layer is marshalling only; see `BUGS.md` and
`docs/plans/2026-11-11-briev-share.md`.

---

## 2. Runtime half — `counsel`

### 2.1 Build pipeline (`build.rs`)

1. Resolve the compiler: `$BRIEVC`, else `brievc` on `PATH`.
2. `brievc build bridge/counsel.bv --library --out $OUT_DIR/briev` — produces a
   real static archive (`libcounsel.a`) plus a PIC `.so`.
3. Link it into the crate (`cargo:rustc-link-search` + `rustc-link-lib=counsel`)
   and re-run when the `.bv` changes (`cargo:rerun-if-changed`).

`src/bindings.rs` holds the `extern "C"` declarations. They are generated with
`brievc bindings bridge/counsel.bv rust --out <dir>` and committed; regenerate
and commit the diff whenever exports change.

### 2.2 What Briev provides

Every export is a pure `Int` ABI function carrying its own contract, so the
prover discharges the machinery itself at build time.

| Export | Returns | Used for |
|---|---|---|
| `add(a, b)` | `a + b` | end-to-end plumbing validation |
| `within(x, lo, hi)` | `1` iff `lo ≤ x ≤ hi` | range obligation |
| `is_positive(x)` | `1` iff `x > 0` | sign obligation |
| `is_nonneg(x)` | `1` iff `x ≥ 0` | sign obligation |
| `eq(a, b)` | `1` iff `a == b` | sampler output comparison |
| `eval_pred(op, a, b, x)` | predicate truth | opcode dispatch: `0=GT`, `1=GE`, `2=LT`, `3=LE`, `4=IN` |

`txn step_up(i, cap) [i < cap][i == cap]` is not exported — it exists to
dogfood a genuinely proved contract (a trivial `[true][true]` contract is a
compile error in Briev).

**Deliberately absent:** a function-pointer orchestrator (`run_checked`). Briev
compiles it, but its C binding advertises a flat `int64_t (*)(int64_t)` while
the backend lowers fn-pointer parameters to a closure/self convention
(`load target from the slot`, call `(slot, arg)`), so a flat host callback
SIGSEGVs. See [`../BUGS.md`](../BUGS.md).

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

## 3. Static half — `counsel-static`

### 3.1 Pipeline

```
check_source(&str)                     (or check_file(&Path))
  │
  ├─ syn::parse_file                     parse Rust
  ├─ translate::translate_file           keep only fns with #[pre]/#[post]
  │    └─ per fn: translate_params → translate_block → assemble defn text
  ├─ brievc::discharge                   run the Briev prover
  │    ├─ find_compiler()                $BRIEVC → PATH → sibling briev-lang builds
  │    ├─ write all defns to one temp .bv
  │    ├─ brievc build <file> --library --verify-bodies
  │    └─ parse_report(stderr)           → proven / unproven / violated
  └─ assemble → Report { verdicts }      classify per declaration
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

Accepted: `i64` parameters and return, integer literals, identifiers, unary
`-`/`!`, arithmetic `+ - * / %`, comparisons, `&&`/`||`, `let`, `if`/`else`,
`return`, a trailing expression, and `#[pre]`/`#[post]`.

Rejected with `Error::Unsupported` (never approximated): calls, method calls,
field access, indexing, slicing, ranges, loops, `match`, pattern `let`s,
`self` methods, non-`i64` types, generics with arguments, `async`.

Rationale: a wrong translation would be a wrong proof.

### 3.4 Discharge and classification

The compiler is **discovered, not assumed**: each candidate is accepted only if
its `--help` advertises `--verify-bodies` (a stale build is skipped rather than
mis-invoked). Candidates: `$BRIEVC`, `brievc` on PATH, sibling
`briev-lang/target/{debug,release}/brievc`.

Verdicts are read from the compiler's stderr:

| Observed line | Outcome |
|---|---|
| `<name>: body can violate its postcondition: …` (inside an `error: --verify-bodies:` block) | `Violated` |
| `warning: --verify-bodies could not prove: <name>: …` | `Unproven` |
| neither line mentions `<name>` | `Proven` |

Exit status handling: a non-zero exit **without** violation lines is surfaced
as `Error::Compiler` — a genuine failure, never an invented verdict.

Advisory warnings about *stdlib* declarations (e.g. `coll_min`, `exponent_val`)
appear in every run and are ignored, because classification only ever asks
about the translated names.

---

## 4. Under the hood: Briev's `--verify-bodies`

Lives in `briev-lang/src/analysis/verify_bodies.rs` (commit `c0ce60dc`), wired
into the compile pipeline behind the flag.

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

`counsel-static` inherits exactly this behaviour, which is why `Unproven` never
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
| Full Rust support | **No** — `i64` subset only |
| Deciding every true contract | **No** — bounds-only, no Z3 arbiter yet |
| Works without a Briev compiler | **No** — `brievc` required at check time |

---

## 6. Where to look

| Path | Contents |
|---|---|
| `bridge/counsel.bv` | Briev machinery: predicates, `eq`, `eval_pred`, `step_up` |
| `build.rs` | compile + link the Briev library |
| `src/bindings.rs` | C ABI declarations (generated, committed) |
| `src/lib.rs` | safe runtime facade |
| `counsel-static/src/translate.rs` | Rust → Briev source |
| `counsel-static/src/brievc.rs` | compiler discovery + discharge + report parsing |
| `counsel-static/src/lib.rs` | `Outcome`, `Report`, `check_source`, `check_file` |
| `tests/smoke.rs`, `counsel-static/tests/valve.rs` | executable specification |
| `BUGS.md` | the cross-ABI callback blocker and the Briev codegen bug |
| `docs/EXAMPLES.md` | worked end-to-end walkthroughs |

## 7. Reproduce

```bash
# Runtime half: builds the Briev machinery, links it, runs the checks
BRIEVC=/path/to/brievc cargo test -p counsel

# Static half: drives the real prover (needs brievc with --verify-bodies)
BRIEVC=/path/to/brievc cargo test -p counsel-static

# Whole workspace
BRIEVC=/path/to/brievc cargo test --workspace
```
