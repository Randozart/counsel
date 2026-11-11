# Plan — Briev `--verify-bodies` (body⇒post proof) + Phase 4 valve

**Date:** 2026-11-11
**Mode:** PLAN (research-first); no code changed.
**Owner decision:** Briev is Randy's language and can be modified. This plan
proposes the Briev-side language improvement that makes Briev *prove* its own
`[pre][post]` on function bodies, and then reuses that same machinery as the
valve into Counsel Phase 4 (static proof of a restricted Rust subset).

---

## 1. Motivation (evidence-backed)

Briev's core claim is *contract-driven verification*, but today it verifies
contract **consistency**, not **body correctness**.

**Empirical (run 2026-11-11):**

```briev
defn lies_about_post(a: Int) [a >= 0][term == 0] -> Int {
    term 5;   // blatantly violates [term == 0]
};
```
- `brievc check body.bv` → OK, exit 0
- `brievc build body.bv --library` → `libbody.a` produced, exit 0

A function returning `5` against a declared postcondition of `0` **compiles and
links**. Source trace confirms why:

| Location | What it does |
|---|---|
| `src/compile.rs:696` | `prove_contract` runs **only** for `ProtocolDef` invariants |
| `src/compile.rs:720–740` | txn/defn get **only** `detect_tautology` (reject `[true][true]`) |
| `src/typechecker/mod.rs:4902` | `validate_contract` rejects tautology/missing only |

The tools to prove bodies **exist but are dormant**: `src/symbolic.rs` is a
Level-2 symbolic executor (`SymbolicState`, `enumerate_paths`,
`satisfies_postcondition`, `eval_symbolic`) with **no production caller in the
compile pipeline** — it's used only by `protocol_graph.rs` (composition-inverse
round-trips) and `fuzzing/concolic.rs`. Likewise `assertion_verify.rs`,
`prove_linear`, `check_convergence` have no non-test compile-pipeline callers.

---

## 2. The gap in `symbolic.rs` (what a body prover needs that it lacks)

`symbolic.rs` is a foundation, **not** a sufficient body prover. Identified
deficits, in the shape the plan must fix:

1. **Return value never bound.** `execute_statement` handles `Assign` and
   `Let` only; `Statement::Term(_)` just pushes state (`enumerate_paths`). The
   function's *output* is never tracked as a variable, so a postcondition
   referencing `term` can never be satisfied. Example that would be wrongly
   rejected even when correct: `[term == a]` with body `term a` — here `term`
   stays `Identifier("term")`, `a` stays `Identifier("a")`, symbolically
   unequal ⇒ false ⇒ rejected.
2. **Two-valued verdict.** `satisfies_postcondition` returns `bool`; `false`
   conflates *provably violated* with *could not prove*. A conservative gate
   that treats `false` as failure would reject correct functions; one that
   treats `false` as pass is unsound.
3. **`path_constraints` are stored but unused.** The state records
   precondition + guard constraints but never consults them to discharge facts
   (`x > 0 ⇒ x ≥ 1`, etc.). This is the gap a Z3 arbiter would close.
4. **`symbolic_less_than` is conservative** (literal-only); inequalities over
   symbolic identifiers are unprovable without constraint reasoning.

So the fix is three-part: **bind the term/return**, return a **tri-state
verdict** (Holds / Violated / Undecided), and **consult `path_constraints`
plus an optional Z3 arbiter** to resolve undecided cases.

---

## 3. The Briev language improvement: `--verify-bodies`

### 3.1 CLI / option plumbing (wiring already traced)

- `BuildOptions` (`src/pipeline.rs:163`): add `pub verify_bodies: bool` (default
  `false` → current behaviour preserved; no existing code breaks).
- CLI parsing (`src/main.rs:278–330`): add `--verify-bodies` alongside
  `--library`/`--keep-all-defns`.
- Second construction site `src/main.rs:922` (bounty): `verify_bodies: false`.

### 3.2 The pass (new, additive)

Add `src/analysis/verify_bodies.rs` — a first-class analysis pass registered in
`src/analysis/mod.rs` (no prototyping; per Golden Rule 10). For each
`txn`/`defn` with a non-trivial contract, when `opts.verify_bodies`:

1. **Bind the precondition** → `SymbolicState::new(pre)`.
2. **Bind the returned value.** Introduce a storage for the output: after
   `enumerate_paths`, treat each terminating path's `Term(Some(expr))` /
   `EndProgram(Some(expr))` output as the value of a synthetic `__result`
   variable (evaluate `eval_symbolic(expr, state)`). Paths that can never
   produce a value (no `term`) must be rejected when a postcondition depends on
   `term`.
3. **Weakest-style walk.** Thread each `SymbolicState` through the body exactly
   as `enumerate_paths` does today (fork on every `Guarded`), tracking
   assignments + `path_constraints`.
4. **Tri-state postcondition evaluation.** Upgrade `satisfies_postcondition` to
   return an enum:
   ```
   enum ProofVerdict { Holds, Violated, Undecided }
   ```
   `Holds`/`Violated` from symbolic equality/inequality; `Undecided` when a
   comparison references a symbolic identifier or unsimplified expression.
5. **Arbiter for Undecided.** When Z3 is available (`is_z3_available()`), build
   an SMT query from `path_constraints ∧ ¬post` via the existing
   `proof_engine::smt::build_contract_query` / `prove_smt_formula`. `Unsat` ⇒
   postcondition holds on that path; `Sat(model)` ⇒ counterexample ⇒
   `Violated`. Unknown ⇒ keep `Undecided`.
6. **Aggregate.** A body is verified iff **every** terminating path is
   `Holds` (no `Violated`, no unresolved `Undecided`). On `Violated` or open
   `Undecided`, produce a `HelpfulDiagnostic` (per house style) naming the
   declaration, the offending path / counterexample, and the fix.

### 3.3 Where it plugs in

In `src/compile.rs`, inside the existing txn/defn contract loop at
`src/compile.rs:720–740` (the `detect_tautology` gate). After tautology
rejection passes, and gated on `opts.verify_bodies`, call the new pass. Keep
it **additive** — the loop already iterates txns and defns and has the
contract; the pass consumes `(&Definition|&Transaction, &Contract)`.

Semantics decision (owner): make this **fail-closed but opt-in** — under
`--verify-bodies`, an unprovable body is an error (that is the point: prove or
be told). Without the flag, current consistency-only behaviour is unchanged.
A future `.s` (strict) profile could enable it by default (ties into the
existing `is_strict` gate at `compile.rs:518`).

### 3.4 Tests (TESTS-OR-IT-DOESN'T-EXIST)

Add `src/analysis/verify_bodies_tests.rs` (or `#[cfg(test)]`) cases:
- Pass: `[a >= 0][term == a + 1] { term a + 1; }`.
- Pass with path split: `[a >= 0][term >= 0] { when a > 5 { term a; }; term 0; }`
  (both paths).
- Reject: `[a >= 0][term == 0] { term 5; }` → must now fail under
  `--verify-bodies`.
- Reject missing term: post depends on `term`, body has no `Term(Some(_))`.
- Undecided-without-Z3 → error-in-form (documented), resolved when Z3 present.
- Existing suite (2900+ tests) must stay green **without** the flag (default off).

---

## 4. Phase 4 valve — Counsel static proof "the way Briev does it"

Once `--verify-bodies` proves body⇒post in Briev, the same gate discharges
verification conditions generated from a **restricted Rust subset**:

1. **Restricted Rust frontend** in Counsel (net-new; uses `syn` in a
   `counsel-static` research crate or a feature). Reject everything but:
   pure-integer functions; scalar `i64`; `if`/`else`/guard + early return + `let`;
   no pointers, borrows, recursion, references, unions, wildcard types.
2. **Translation → Briev `Expr`/`Statement`**
   (`briev_compiler::ast` is `pub`, `src/lib.rs:30`), i.e. we author the
   translated body as Briev AST programmatically (or via Beast round-trip,
   `beastpack`). Because `--verify-bodies` consumes (body, contract), Counsel
   builds the FC body + `[pre][post]` and hands them to the SAME pass.
3. **Discharge.** The generated VC flows through the new pass's
   `path_constraints + Z3` machinery — reusing `prove_smt_formula`/`Expr`
   (`proof_engine` is `pub`, `src/lib.rs:83`). The heavy prover is already
   built; the new engineering is (a) the Rust-subset frontend, (b) the
   Rust→Briev-Expr translation. That is the *lighter-than-Kani* claim's real
   basis — reject the hard surface, keep the WP kernel small.

This makes Counsel's static phase "the way Briev does it" literally: the SAME
gate that proves Briev functions also proves the translated Crystals, and it is
the improvement Briev itself was missing.

Trade-off note: Z3 is an **external binary** (PATH or managed dir) for both the
Briev gate and the Phase 4 valve; a consumer with no Z3 falls back to
heuristic-only (less precise) proofs. Documented, not a blocker.

---

## 5. Optional: cross-ABI fn-ptr fix (low priority, decoupled)

Separate from the above. Root cause of the Phase-1 `run_checked` SIGSEGV:
`emit_call_ptr` (`src/backend/llvm/intrinsics.rs:1455`) double-derefs the
fn-ptr parameter (loads the target *from* the param slot, calls `(slot, arg)`
— a closure/self convention), while the C binding generator advertises a flat
`int64_t (*)(int64_t)`. The two disagree; a flat host fn panics (SIGSEGV).

Fix direction (owner decision needed): make the crossing ABI consistent — either
(a) the backend calls the flat signature for plain `fn` params (no slot
deref, no self arg), or (b) the binding generator emits the slot/self
signature and Counsel builds a thunk. The plan's recommendation: **do not block
verify-bodies or Phase 4 on this**; predicates already cross as *data* and that
works. Defer the ABI decision to a separate minimal spike. (Documented in
`BUGS.md`.)

---

## 6. Work plan (ordered)

| # | Task | File(s) | Gate |
|---|---|---|---|
| 1 | Add `verify_bodies: bool` to `BuildOptions` + both construction sites | `pipeline.rs:163`, `main.rs:278–330`, `main.rs:922` | build |
| 2 | Add `--verify-bodies` CLI flag | `main.rs` | `brievc --help` |
| 3 | New `verify_bodies` analysis pass (bind term, tri-state, constraints+Z3) | `src/analysis/verify_bodies.rs` | `cargo test --lib` |
| 4 | Wire call into txn/defn loop | `compile.rs:720–740` | `cargo test --lib` |
| 5 | Pass + reject tests (incl. regression: existing suite green, flag off) | tests | `cargo test` |
| 6 | Additive regression: whole repo conformance sweep still passes | — | `cargo test` |
| 7 | (Counsel) Probe gateway: build a `brievc --verify-bodies` Briev function that proves a real contract | counsel-crate | `cargo test` in counsel |
| 8 | (Counsel, Phase 4) restricted-Rust→Briev-Expr translator spike + VC discharge | `counsel-static` | minimize + `brievc --verify-bodies` |

---

## 7. Risks & trade-offs

- **Default-off keeps the 2900+ suite green** — the change is strictly additive.
- **False rejects** are the failure mode to design against: the tri-state +
  Z3-arbiter design ensures we only reject on a concrete counterexample
  (`Violated`), never on mere `Undecided` (that becomes an explicit
  "could not prove" error, which the owner may choose to grade as warning vs
  error depending on strictness appetite).
- **Soundness boundary:** symbolic executor does not model memory/pointers.
  Bodies containing pointers, `Alloc#`, external `frgn`, `Call` to unanalysed
  functions must be classified `Unknown`/skipped under verify-bodies (never
  wrongly proven). The pass should declare coverage per body and skip unknowns.
- **Z3 dependency:** improve `managed_z3_path`/install story or document; both
  the Briev gate and Phase 4 prefer it but degrade gracefully.

---

## 8. Success criteria

1. `defn lies_about_post [a>=0][term==0] { term 5; }` **fails** under
   `brievc --verify-bodies`, with a helpful diagnostic; **still compiles**
   (consistency-only) without the flag.
2. A correct body (`[a>=0][term==a+1]{ term a+1; }`) passes under the flag.
3. Existing test suite green with the flag off (identical behaviour).
4. Counsel Phase-4 probe discharges a translated VC through the same gate.