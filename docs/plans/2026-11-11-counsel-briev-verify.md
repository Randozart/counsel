# Counsel — Active Plan (Phases 0–4)

**Date:** 2026-11-11
**Status:** Phase 0 complete (scaffold spike) — `cargo test` green, contracts proved, praetor clean, committed. Next: Phase 1.
**Feasibility/decision source:** `../BRIEV-RUST-VERIFY-PLAN.md`

## Purpose

Build **Counsel**, a Cargo crate that helps Rust developers do *basic*
pre/post-condition verification on their own code — with the verification
machinery **authored in Briev** (`.bv`), compiled to a native library, and
linked into the crate via the proven `brievc build --library` +
`bindings` + `build.rs` path.

Positioned as **lighter than Kani**: runtime obligation enforcement plus a
cross-verification sampler, not a full model-checker over the Rust memory
model.

## Non-goals (first deliverable)

- **Static proof** of Rust functions (Ambition B). Briev's prover runs over
  Briev AST at compile time; it does not consume Rust source. Not shipped as
  the first crate.
- A full prover in the crate.

## Architecture

```
counsel-crate/
├── bridge/counsel.bv        # Briev machinery: obligation evaluator,
│                            #   bound/env handling, reporter, sampler
├── cookbook                 # (later) contract DSL / doc
├── src/                     # Rust facade
│   ├── lib.rs               #   safe API (panic/Result reporters)
│   └── macros/              #   (Phase 1) #[contracts] proc-macro
├── build.rs                 # runs brievc build --library + links
├── Cargo.toml
└── docs/plans/…             # this file
```

Two-priority predicate transport:
- **A1** Rust-closure obligations (predicates as fn pointers across the C ABI).
- **A2** pure-Briev predicate AST (the flagship "written in Briev" claim) — later.

## Phase 0 — Scaffold spike (current)

Goal: prove the full build path end-to-end with happy-path exports; establish
the repo and the strict-standards discipline.

1. Repo init (`counsel-crate/`, `git init -b main`). ✅
2. `AGENTS.md` (adapted strict standards). ✅
3. This plan.
4. `bridge/counsel.bv` — minimal predictable exports:
   - `add(a: Int, b: Int) -> Int` — clean C ABI, plumbing validation.
   - `within(x: Int, lo: Int, hi: Int) -> Int` — a runtime check predicate
     helper (x within [lo, hi] → 1 else 0), Briev-computed. Seed of the
     obligation evaluator.
   - One `txn` with a real, provable `[pre][post]` (e.g. an ascending step)
     to prove Briev's own prover agrees on our code.
5. `Cargo.toml` + `src/lib.rs` + `src/bindings.rs` + `build.rs` wiring
   (modeled on `briev-lang/examples/glue-host/rust-host/`).
6. Smoke test: `cargo test` builds, links, and calls the Briev exports through
   the C ABI. Contracts prove. Bindings checked in.

Exit criteria: `cargo test` green; `brievc build --library` proves every
contract on `counsel.bv`; no `todo!`/stubs; commit.

**Status: DONE (2026-11-11).** `cargo test` 3/3 green; `cargo build` clean (no
warnings); `praetor validate --warn --target src` and `--target .` both PASS;
`step_up` contract `[i < cap][i == cap]` proved by Briev's prover; bindings
checked in.

## Phase 1 — Ambition A1 (Briev-computed checks, Rust composition)

> **Pivot (2026-11-11):** the original A1 (Rust closures crossing the C ABI as
> flat `extern "C" fn`) is blocked by a Briev runtime ABI bug — see BUGS.md.
> The cross-ABI fn-ptr param compiles but the backend calls `(slot, arg)`
> (closure convention) while bindings advertise a flat `(*)(int64_t)`;
> result SIGSEGV. Reliable form adopted below.

- Briev: check predicates over a pure `Int` ABI (`within`, `is_positive`,
  `is_nonneg`), each carrying its own contract and proven at build time.
- Rust facade: `contract(pre, body, post, x)` combinator composes pre/body/
  post; predicate closures delegate truth values to the Briev checks
  (`positive`, `nonneg`, `in_range`). Orchestration lives in Rust; each
  obligation's truth value is Briev-computed.
- Exit: a user fn with `pre: x>0`, `post: ret>=0`; a violating call returns a
  `Verdict::Fail` naming which obligation fired.

**Status: implemented; verification + commit pending.**

## Phase 2 — Cross-verification sampler

- Port the `:=`/`cross_verify_samples` idea: `verify(original, shadow, n)`
  runs interleaved samples, compares (IO-equivalence), returns a verdict.
- The "Briev does it" signature feature; echoes the shadow-benchmark model.

## Phase 3 — Ambition A2 (pure-Briev predicate AST)

- Tiny contract DSL: `pre(Gt(Var(x), Lit(0)))`, serialized to the Briev side,
  evaluated *in Briev*. Removes the "Rust closures do the checking" caveat.

## Phase 4 — stretch slice of Ambition B (optional / research)

- Restricted Rust subset → VC → Briev `proof_engine`/Z3. Prototype only.

## Standards discipline (replicates briev-lang AGENTS.md)

- Contract-first: never weaken `[x > 0]` to `[true]`.
- Intent comment before every function (Rust and Briev).
- Flat control flow, ≤2 nesting, no deep `else if`.
- Tests or it doesn't exist: `cargo test --lib` green before every commit.
- Always finish: no `todo!()`, `unreachable!()`, stubs.
- Continuous commits, targeted `git add`, never amend / never `git checkout --`.
- `praetor validate --warn --target <DIR>` on changed directories.
- Apache-2.0 WITH LLVM-exception headers on new source.
- Docs-and-plans stay current; outdated docs are bugs.

## License

Apache-2.0 WITH LLVM-exception (matching parent project). Compiled Briev
output is exempt from notice/attribution obligations.