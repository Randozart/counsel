# Research — Phase 4 (Static Proof) Feasibility & Briev Capability Audit

**Date:** 2026-11-11
**Status:** Research complete. One Briev incapacity identified (std.), with
evidence; recommendation + language-improvement consideration below.

## Scope

Phase 4 of Counsel was the optional/research "stretch-B": a **static** proof
path that verifies Rust contracts the way Briev verifies its own — i.e. by
discharging pre/post obligations through Briev's prover, rather than the
runtime enforcement shipped in Phases 1–3. This audit asked:

1. Can Briev's `proof_engine` be reused as a library to discharge
   verification conditions (VCs)?  → **Yes.**
2. Does Briev itself contain a general *body-satisfies-postcondition*
   (weakest-precondition) prover wired into compilation?  → **No — the
   decisive finding.**
3. What is the real cost of the static path, and is it viable for a
   "lighter-than-Kani" checker?

## What Briev's proof_engine is and isn't

### Reusable (yes)

- `briev-compiler` is a proper library (`crate-type = ["lib", "staticlib"]`,
  `src/lib.rs`); `pub mod proof_engine` and `pub mod ast` are reachable from the
  crate root (`src/lib.rs:83`, `:30`). An external Rust crate can depend on it.
- Public prover surface (`src/proof_engine/mod.rs`):
  - `prove_contract(pre, post, params, explicit) -> Result<(), Vec<ProofError>>`
  - `prove_smt(formula: &Expr, _params) -> SmtResult`
  - `prove_smt_formula(&str, timeout) -> SmtResult`
  - helpers: `check_satisfiable`, `split_and`, `detect_tautology`,
    `extract_bound_from_postcondition`, `check_convergence`.
- `Expr`/`Statement` AST is public (`src/ast/expr.rs:38` `pub enum Expr`), so a
  caller can either build Briev AST programmatically or round-trip Beast
  (`beastpack::{serialize,deserialize}`).

### Z3 is an external binary, not a crate dependency

`src/proof_engine/smt.rs` shells out to a `z3` binary — found on `PATH` or a
managed dir (`~/.local/share/briev-compiler/bin/z3`, via `is_z3_available()` /
`managed_z3_path()`). No `z3` crate is in `Cargo.toml`. **A consumer reusing
the prover must have a `z3` binary installed** (or her heuristic-only, less
precise fallback applies). Absent Z3, `prove_contract` falls back to heuristic
`check_satisfiable` — violations deny, inconclusive allows.

## THE DECISIVE FINDING — Briev does not prove body ⇒ postcondition

`prove_contract` checks that a contract is **internally consistent /
satisfiable**, and rejects **tautologies** — it does **not** prove that a
function's *body* guarantees its *postcondition*:

- `src/compile.rs:696` calls `prove_contract` **only** for `ProtocolDef`
  invariants.
- For `txn`/`defn`, `src/compile.rs:727` applies only `detect_tautology`
  (rejects `[true][true]` and equivalent).
- The typechecker (`src/typechecker/mod.rs:4902` `check_contract_obligations`
  → `validate_contract`) also only rejects tautologies and missing contracts.

**Empirical proof** (run 2026-11-11):

```briev
// Pre [a >= 0]; post [term == 0]; but the body always returns 5.
defn lies_about_post(a: Int) [a >= 0][term == 0] -> Int {
    term 5;
};
```

- `brievc check body.bv` → `OK`, exit 0
- `brievc build body.bv --library` → `libbody.a` produced, exit 0

A body that blatantly violates its own postcondition **compiles and links
successfully**. So Briev's current verification is contract *consistency +
termination + tautology rejection*, not general body-correctness.

### The dormant WP machinery

`src/symbolic.rs` implements Level-2 symbolic execution with exactly the pieces
a body-prover needs: `SymbolicState::new(precondition)`, `enumerate_paths`,
`satisfies_postcondition(post, state)`, `eval_symbolic`. But grep shows these
have **no production callers in the compile pipeline** — `symbolic.rs` is used
only by `protocol_graph.rs` (composition-inverse round-trip proofs) and
`fuzzing/concolic.rs`. Likewise `assertion_verify::verify_true_assertion`,
`prove_linear`, and `check_convergence` have no non-test compile-pipeline
callers. The machinery exists; the compiler does not invoke it to prove bodies.

## Implication for Phase 4

"The way Briev does it" (static body proof) **does not exist in Briev yet**.
Implementing Phase 4 therefore means building a net-new **verification-condition
generator** (restricted Rust subset → WP/VC → SMT), then discharging the
generated VCs through Briev's reusable `prove_smt_formula` (which does work and
is reachable). The prover is the easy, already-built half; the VC generator is
the new engineering.

Cost estimate (restricted pure-integer, no pointers/borrows/recursion subset):
- **VC generation** (weakest precondition over `if`/`else`/early-return/`let`):
  small-to-moderate — a focused WP walker over a tiny Rust surface (~hundreds of
  lines), not a full model-checker. This is the *lighter-than-Kani* claim's real
  basis: reject everything but pure integer functions, and the VC generator
  stays small.
- **Discharge**: call Briev `proof_engine::prove_smt_formula` (or build
  `Expr` and call `prove_contract`) — already implemented.
- **Ergonomics**: needs a Rust frontend (`syn`) just to extract the restricted
  subset — a real added dependency flange the runtime crate avoided.

## Briev incapacity → language-improvement consideration

**Incapacity:** Briev ships strong contract *consistency/termination* checking
and an *unwired* symbolic-execution body prover (`symbolic.rs`), but has **no
general weakest-precondition prover that verifies "given pre, the body's
execution establishes post"** at compile time. The `symbolic.rs` engine looks
designed for exactly this; it is simply not connected to the `txn`/`defn`
compile gate.

**Suggested language improvement (owner decision):** wire `symbolic.rs`
(`enumerate_paths` + `satisfies_postcondition`, or a Z3-discharged WP walker
over the body) into the contract-checking path for `txn`/`defn`, under a
strict-profile/`--verify-bodies` flag to avoid breaking existing code that
relies on the current consistency-only semantics. That would make Briev actually
*prove* `[pre][post]` on its own functions — which would both strengthen Briev
itself and enable Phase 4's static proof "the way Briev does it" nearly for
free (reusing that gate for translated VCs).

## Briev capabilities confirmed (non-issues for this route)

- Cross-ABI **export of pure Int-ABI functions** — fully reliable (Phases 0–3).
- **Cross-ABI function-pointer parameters** — NOT reliable (SIGSEGV; see
  `BUGS.md`). Predicates must cross as *data*, as Counsel does.
- Storing/comparing Int predicates (`eval_pred`, `eq`) — reliable.

## Citations (evidence, file:line)

- Library + public modules: `briev-lang/Cargo.toml` (`[lib]`, `crate-type`),
  `src/lib.rs:30` (`pub mod ast`), `:83` (`pub mod proof_engine`).
- Z3 external: `src/proof_engine/smt.rs:193` (`is_z3_available`),
  `:211` (`managed_z3_path`); no z3 in `Cargo.toml`.
- Prover surface: `src/proof_engine/mod.rs:22` (`prove_contract`),
  `src/proof_engine/smt.rs:22,56` (`prove_smt`/`prove_smt_formula`).
- prove_contract scoped to ProtocolDef: `src/compile.rs:696`.
- txn/defn only tautology: `src/compile.rs:727`.
- typechecker consistency-only: `src/typechecker/mod.rs:4902`.
- Dormant WP machinery: `src/symbolic.rs:89,440,348`; callers only
  `src/analysis/protocol_graph.rs`, `src/fuzzing/concolic.rs`.
- Empirical body-violates-post: probe `defn lies_about_post [a>=0][term==0] {
  term 5; }` compiled OK (`brievc check` exit 0; `brievc build --library`
  produced `libbody.a` exit 0).