# Plan — Where the Briev actually is (and why it's 114 lines)

**Date:** 2026-11-11
**Trigger:** owner observation — "suspiciously little .bv code."
**Status:** diagnosis complete; options below; no code changed yet.

---

## 1. The measurement

`counsel-crate`, tracked files only:

| Language | Lines | Files |
|---|---:|---:|
| Briev (`.bv`) | **114** | 1 |
| Rust (`.rs`) | **1519** | 8 |

That is ~7% Briev by line count. The claim in `README.md` / `docs/ARCHITECTURE.md`
("the verification machinery is authored in Briev") is therefore **overstated**
and should be corrected regardless of what else we do (see §6).

### What is actually in Briev (`bridge/counsel.bv`)

| Declaration | Role |
|---|---|
| `add(a, b)` | plumbing validation |
| `within(x, lo, hi)` | range predicate |
| `is_positive(x)` | sign predicate |
| `is_nonneg(x)` | sign predicate |
| `eq(a, b)` | equality (sampler comparison) |
| `eval_pred(op, a, b, x)` | opcode dispatch `0=GT 1=GE 2=LT 3=LE 4=IN` |
| `txn step_up(i, cap)` | not exported — proves a real `[i < cap][i == cap]` contract |

That is a **predicate layer**, not machinery.

### What is in Rust, by role

| File | Lines | Why it is Rust |
|---|---:|---|
| `src/lib.rs` | 253 | facade + pre/body/post orchestration + sampling loop |
| `counsel-static/src/translate.rs` | 461 | parses Rust (`syn`), emits Briev text |
| `counsel-static/src/lib.rs` | 237 | `check_source`, `Report`, `Outcome` classification |
| `counsel-static/src/brievc.rs` | 180 | compiler discovery, process spawn, stderr parsing |
| `tests/*` | 307 | tests |
| `build.rs` + `src/bindings.rs` | 81 | build glue and ABI declarations |

### And the machinery itself

The prover is **not in this repo at all** — it is in the compiler:

| File (briev-lang) | Lines | Language |
|---|---:|---|
| `src/analysis/verify_bodies.rs` | 724 | Rust |
| `src/symbolic.rs` | 737 | Rust |

So the honest accounting for "verification machinery": **1,461 lines of Rust**
in `briev-lang`, plus **114 lines of Briev** predicates, plus ~1,100 lines of
Rust integration.

(For context, `briev-lang` itself is 431 `.rs` files to 154 `.bv` files — the
compiler is Rust; that is expected, but it means "authored in Briev" can never
describe the prover today.)

---

## 2. Why the Briev share is thin — three causes

### Cause 1 — cross-ABI callbacks are broken (the dominant cause)

Briev lowers a function value to a heap env block `[fn_ptr, cap1..capN]` and
calls it through that slot (`(slot, args)`), not as a flat C pointer — see
`briev-lang/src/backend/llvm/emit_expr.rs:1940-1950` (the `Expr::Lambda` arm
documents the `[fn_ptr, cap1..capN]` representation) and the closure-call
lowering that loads the code pointer *from* the slot.

A host C function pointer is flat, so any exported Briev function taking a
`(Int) -> Int` parameter SIGSEGVs when the host calls it (documented in
`BUGS.md`). Consequence: **everything that must call back into Rust had to be
rewritten in Rust** — precisely the two pieces designed to live in Briev:

- `run_checked(pre, f, post, x)` — the obligation orchestrator (written, then reverted);
- `verify(original, shadow, n, gen)` — the cross-verification sampler loop.

Those are the parts that make Counsel a *verifier* rather than a predicate
table, and they are Rust only because of an ABI defect.

### Cause 2 — the prover belongs to the compiler, and the compiler is Rust

`--verify-bodies` (1,461 lines) lives in `briev-lang` and is Rust. Porting it
to Briev is the language's self-hosting endgame, not a crate-level task.

### Cause 3 — some work is irreducibly Rust

Parsing Rust (`syn`), spawning `brievc`, reading its stderr, and cargo
integration cannot be expressed in Briev: they operate on Rust source and host
processes. Roughly 641 lines (translate + brievc) are in this category and
should stay Rust.

---

## 3. Options

| Option | What | Effort | Briev gained | Risk |
|---|---|---|---|---|
| **E. Correct the claim** | scope the docs: "predicate core in Briev; prover is part of the Briev compiler" | ~30 min | none (honesty) | none |
| **A. Fix the FFI callback ABI** | make `fn`-typed FFI params use the flat C convention (or auto-wrap host pointers into a slot at the boundary) | 1–2 days (briev-lang backend + tests) | `run_checked` + `verify` return to Briev; real orchestration authored in Briev | medium — ABI change; needs a regression gate |
| **B. Grow the predicate library** | conjunctions, disjunctions, multi-variable, implication, interval algebra in `.bv`; Rust `Op` becomes a marshaller | 1 day | +150–250 lines of genuine Briev; predicate semantics owned by Briev | low |
| **C. Move report classification to Briev** | parse `<name>: reason` lines in Briev (needs `%state` + String ABI) | 1–2 days | +100 lines; stderr interpretation in Briev | medium — introduces state lifecycle into a currently stateless crate |
| **D. Port the prover to Briev** | `symbolic.rs` + `verify_bodies.rs` become `.bv` | weeks–months, multi-milestone | the actual claim becomes true | high — depends on solver ergonomics; blocks on self-hosting progress |

### Trade-off notes

- **A is the highest-leverage option**: it is a *language* fix (it benefits any
  host embedding, not just Counsel), and it restores the design we had to
  abandon. It also converts ~60 lines of Rust orchestration into Briev, but its
  real value is architectural: ordering and short-circuiting semantics become
  Briev-authored.
- **B** is additive and safe; it makes the Briev layer meaningful rather than a
  switch statement over an opcode.
- **C** buys little for its risk: stderr parsing is host-glue work, and moving
  it forces `needs_state` plumbing (`glue/export.rs:125` computes it) into a
  crate that is currently state-free.
- **D** is the only option that makes "machinery authored in Briev" literally
  true, but it is a language-programme milestone, not a crate task.

---

## 4. Recommendation (ordered)

1. **E now** — correct the claim in `README.md`, `docs/ARCHITECTURE.md` and the
   crate description; publish the real split as a table. Cheap, and prevents the
   repository from overstating itself.
2. **A next** — fix the FFI callback ABI in `briev-lang` (feature request:
   "exported functions taking `fn` parameters use the C ABI, not the closure
   slot convention"), then restore `run_checked` and move the sampler loop into
   Briev. Ship with a regression test that a host C callback works.
3. **B then** — grow `counsel.bv` into a real predicate algebra.
4. **D as a tracked track** — a self-hosting milestone for the prover, listed
   but not scheduled here.

## 5. Success criteria

- After E: no doc claims the prover is authored in Briev; the split is stated
  with numbers.
- After A: `bridge/counsel.bv` contains the verification *orchestration*
  (`run_checked`, sampler), a Rust host can pass C callbacks, and the shared
  library still passes the 21-test suite.
- After B: predicate composition (AND/OR/range/implication) is expressible
  without new Rust arms.

## 6. Doc corrections required (part of E)

| File | Current claim | Corrected |
|---|---|---|
| `README.md` | "the verification machinery **authored in Briev**" | "the predicate core is authored in Briev; the body prover is part of the Briev compiler" |
| `docs/ARCHITECTURE.md` | same framing in §1/§2 | add a "who owns what" table with the measured split |
| `counsel-static/Cargo.toml` description | "discharged through Briev's body prover" | accurate already |
| `.github`/repo description | "verification machinery authored in Briev" | scope to predicates |

## 7. Non-goals

- Porting `translate.rs` or `brievc.rs` to Briev (they are host-side by nature).
- Rewriting the prover in this workstream (that is D).
