# Counsel — Agent Guidelines

**2026-11-11:** Operating manual for the Counsel crate: a Cargo crate that helps
Rust developers do *basic* pre/post-condition verification on their own code,
with the verification machinery **authored in Briev** and compiled to a native
library via the proven `brievc build <f.bv> --library` + `bindings` + `build.rs`
path. Positioned as **lighter than Kani** (runtime obligation enforcement +
cross-verification sampler, not a full model-checker).

These guidelines mirror the strict standards of `briev-lang/AGENTS.md` (the
parent language project), adapted to a library crate rather than a compiler
(so optimizer-only rules like "additive-only codegen" do not apply; the
correctness/contract/quality rules do, in full).

## Operating Contract

Zero tolerance for "probably fine". Every feature must be correct for all
inputs, not just the test in front of you. Every decision passes three
questions:

1. **Does this make the crate more general, or special-case one usage?**
   A check that only understands `i64` arguments inherits a lifetime of
   `u128`/struct/value-type requests. Model contracts over a general
   representation.
2. **Does this add knowledge the crate must carry forever, or push it into
   Briev stdlib / the Rust facade where it can evolve?** Logic that is Briev's
   job lives in `.bv`; logic that is Rust's ergonomics lives in Rust. Never
   duplicate the same rule in both.
3. **If this were the only rule left, would the architecture still hold?**
   Removing any one rule must not break the others.

Patches are unacceptable. No "go fast and break things."

## Golden Rules

1. **CONTRACT-FIRST**: Contracts are the source of truth. In the Rust facade
   AND the Briev machinery, never weaken `[product > 0]` to `[true]` — fix the
   code, not the contract. The whole point of this crate is that contracts are
   real; a trivial `[true][true]` contract is rejected by Briev's own prover.
2. **AUTHOR THE MACHINERY IN BRIEV**: The verification engine (obligation
   evaluator, bound/environment handling, failure reporter, cross-verification
   sampler) is written in Briev `.bv` and compiled to native. Rust is the thin
   facade (macro + bindings). Never re-implement the engine's logic in Rust
   "because it's easier" — that is the ambition dying by a thousand shortcuts.
3. **INTENT COMMENT BEFORE EVERY FUNCTION**: Every public function (Rust and
   Briev) carries a doc/intent comment stating what it guarantees. A missing
   intent comment is a hard error.
4. **FLAT CONTROL FLOW**: max 2 nesting levels. Use guard clauses, early
   returns, `?`. Deeper logic goes in named helpers. No `else if` chains
   deeper than one level. (Matches the parent project.)
5. **NO TYPE-NAME MATCHING-AS-DISPATCH**: In the Rust facade, never dispatch
   on `TypeId`/string type names to decide verification strategy. Model value
   kinds via a protocol or trait so the engine stays general.
6. **TESTS OR IT DOESN'T EXIST**: Every feature and code path needs tests.
   `cargo test --lib` (and the integration tests) green before every commit.
7. **ALWAYS FINISH**: No `todo!()`, `unreachable!()`, `// TODO:`, or stubs in
   committed code. Every feature wired facade → machinery → bindings → tests.
8. **NEVER DISCARD UNCOMMITTED WORK**: `git checkout --`, `git restore`, and
   `git checkout .` DESTROY work permanently — never use them. Commit your own
   changes with targeted `git add`; never stash others'. `git reset HEAD` is
   safe (unstaging only).
9. **CONTINUOUS COMMITS**: Commit after each logical step; commit when a step
   is complete and tests pass (do not ask). `git add` only intended files;
   never amend.
10. **DOCS IN PLANS**: Write `docs/plans/YYYY-MM-DD-<topic>.md` before
    plan-driven work. Outdated docs are bugs.
11. **VERIFIER IS VERIFIED**: The Briev machinery's own functions carry
    contracts. Satisfy Briev's contract prover (`[true][true]` and tautologies
    are compile errors). If Briev cannot prove a contract, simplify the code —
    never weaken the contract.
12. **COMMON RULE, ONE HOME**: A pattern appearing 3+ times becomes a
    centralized helper (in Briev stdlib or the Rust facade). Grep ALL call
    sites when changing a helper's behavior.

## Working Rules

- **License**: Apache-2.0 WITH LLVM-exception (matches the parent project's
  header). New source files get the LLVM-style boxed header
  (`SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception`). Compiled output
  of Briev programs is EXEMPT from notice/attribution obligations.
- **Helpful diagnostics**: every failure (Rust panic or Briev report) states
  what was wrong, which obligation failed, and the concrete fix. Terse,
  factual, kind.
- **HashMap/iteration determinism**: any iteration that affects emitted output
  or observable behavior is sorted/deterministic.
- **Per-commit checklist**:
  - `cargo test` (lib + integration) green.
  - `cargo build` no new warnings.
  - `praetor validate --warn --target <DIR>` on changed directories (the
    **directory**, never a file: `--target src/foo.rs` silently passes, exit 0).
    Pre-existing diagnostics in untouched files are the project baseline; the
    gate is "no NEW diagnostics in changed files".
  - Re-run the Briev machinery through `brievc build --library` to confirm the
    `.bv` still compiles and its own contracts still prove.
  - Update docs/architecture or docs/plans if API/behavior contracts changed.

## Commands

- **Build/Briev machinery**: `BRIEVC=... brievc build bridge/counsel.bv --library --out <dir>`
  (the `build.rs` does this; `BRIEVC` env selects the compiler, default `brievc`
  on PATH).
- **Generate bindings**: `brievc bindings bridge/counsel.bv rust --out <dir>`
  → `<dir>/counsel-bindings/briev_bindings.rs` (checked in; regenerated and
  committed when exports change).
- **Test**: `cargo test` (runs the build.rs → links → smoke/integration tests).

## Reference Index

| Resource | Location |
|----------|----------|
| Feasibility study + decision record | `../BRIEV-RUST-VERIFY-PLAN.md` |
| Active plan (phases 0–4) | `docs/plans/2026-11-11-counsel-briev-verify.md` |
| Regex/stdlib decisions | Briev: `docs/architecture/` in `briev-lang` |
| Rust host GLUE reference | `briev-lang/examples/glue-host/rust-host/` |