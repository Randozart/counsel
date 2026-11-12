# Intended cases — `#[case(...)]` as checked examples

**Date:** 2026-11-12
**Status:** design (no code changed).
**DECIDED (2026-11-12):** the shipped surface is `#[pre]`, `#[post]`,
`#[case]` — Counsel's own syntax. The `#[intent(...)]` prose attribute proposed
below is **dropped**: prose that is never checked does not belong in a crate
whose promise is that unproven is never a pass. The `#[case]` mechanism in this
document is unaffected. See
`2026-11-12-contract-attribute-naming.md` §8 for the decided surface.
**Question (owner):** is there space for an `intend`/`verify` block that lists the
intended cases?

**Short answer: yes — and it needs no compiler or prover change**, because the
translator can instantiate each case into a fully literal Briev `defn`, which the
existing prover already decides.

---

## 1. Why it fits the machinery

Today's shapes:

```rust
#[pre(a >= 0)]
#[post(term == a + a)]
fn double(a: i64) -> i64 { a + a }
```

`Unproven` is common because the prover reasons with interval bounds and
structural equality only — it has no numeric value to work with. **A concrete
case supplies exactly that**: substitute the literal and the whole body collapses
to a constant, which the prover decides outright.

The value is therefore not "more tests" but **converting `Unproven` into a
definite answer** for the inputs the author actually cares about, and producing
real counterexamples when they are wrong.

---

## 2. Feasibility findings (from the code)

### 2.1 A contract cannot reference a body local

`briev-lang/src/typechecker/mod.rs:2860-2872` elaborates the contract with the
parameters bound (plus `term` for the post) *before* the body's locals exist. So
this shape is unavailable:

```briev
defn f_case() [term == a + a] -> Int {   // `a` is not in scope
    let a: Int = 5;
    term a + a;
};
```

### 2.2 Pinning via a precondition does not decide arithmetic

`--verify-bodies` records `Eq` as a **point interval** (`update_bounds`), but
`eval_symbolic` does not consult bounds — it returns `Identifier("a")`. So
`[a == 5][term == a + a]` compares `Binary("+", a, a)` against a literal and
stays **Undecided**. (This could be fixed by folding pinned identifiers into
literals — a general prover improvement, worth doing someday, but not required
here.)

### 2.3 Therefore: substitute literals in the translator

`counsel-static` already walks the Rust AST to emit Briev text. Give that walk a
substitution map `param → literal` and a case becomes:

```briev
// #[case(5 => 10)]
defn double__case0() [true][term == 5 + 5] -> Int {
    term 5 + 5;
};
```

The prover evaluates `5 + 5` to `10`, matches `term == 10`, and returns **Holds**
— with no compiler change, no prover change, and no new dependency.

---

## 3. Proposed syntax

```rust
#[pre(a >= 0)]
#[post(term == a + a)]
#[case(5 => 10)]
#[case(0 => 0)]
fn double(a: i64) -> i64 { a + a }
```

| Attribute | Machine-checked | Purpose |
|---|---|---|
| `#[case(input => expected)]` | **yes** | a concrete point the contract must hold at |

Naming notes: `#[verify]` collides with the crate's own verbs (`check_source`,
`verify_samples`) and sounds like "run the tool". `#[expect]` is **out** — Rust
1.81 stabilised `#[expect(lint)]` with different semantics. `#[case]` says
exactly what it is and is naturally repeatable.

`#[intent]` stays free-text on purpose: prose that is never checked must not
look like a contract.

---

## 4. Semantics — and the honesty rule that governs them

**A passing case is not a proof.** This must be structural, not a doc footnote:

| Outcome | Meaning |
|---|---|
| `Proven` | symbolic: the postcondition holds on every path |
| `ProvenForCase { input }` | holds **at that input only** |
| `Violated` | a concrete counterexample exists |
| `Unproven` | decidable body, undecided obligation |
| `CaseFailed { input, expected, actual }` | the case's `=> expected` disagrees with the body |

- `Report::all_proven()` must count **only** symbolic `Proven`, so a function
  with cases but no general proof never reports green.
- The CLI/report prints cases in their own section, labelled as examples.

### Bonus: cases detect a wrong *contract*

If the body returns `10`, the case says `10`, but the postcondition says
`term == 11`, the prover reports `Violated` while the case disagrees — a strong
signal that the **contract is wrong**, not the code. That diagnostic is worth
building: "case (5 => 10) matches the body, but the postcondition is false here
— check `#[ensures]`."

Cases also catch **vacuous contracts**: a contract no case can satisfy is
usually mis-written.

---

## 5. Translation shape

For `fn double(a: i64) -> i64 { a + a }` with `#[case(5 => 10)]`:

```briev
// general goal — unchanged, may be Unproven
defn double(a: Int) [a >= 0][term == a + a] -> Int { term a + a; };

// instantiated case — decidable
defn double__case0() [true][term == 5 + 5] -> Int { term 5 + 5; };
```

Substitution rules:
- Replace **only parameters**, and only where not shadowed by a `let` of the
  same name (the subset allows `let a = …;` — the walk must track shadowing).
- Pre and post are substituted identically, so the author writes the contract
  once.
- The `=> expected` right-hand side is *not* substituted into the goal — it is
  compared for the cross-check in §4.

---

## 6. API / report changes

- `counsel-static/src/translate.rs`: `translate_expr(…, subst: &Subst)`; a new
  `translate_cases(fn) -> Vec<TranslatedFn>` emitting `__caseN` defns.
- `Outcome` gains `ProvenForCase` / `CaseFailed`; `Report` gains
  `cases: Vec<CaseVerdict>` (or `verdicts` carries a `kind`).
- `Report::all_proven()` counts symbolic proofs only.
- `#[intent("…")]` is carried on `FnVerdict` for display.

---

## 7. Tests

1. `#[case]` on a true contract → `ProvenForCase`; general goal still `Unproven`.
2. Case with wrong `=> expected` → `CaseFailed` naming expected/actual.
3. Case whose body violates the contract → `Violated` with the concrete values.
4. `all_proven()` stays false when only cases pass (the honesty gate).
5. Shadowing: `fn f(a: i64) -> i64 { let a = 1; a }` with `#[case(5 => 1)]` —
   substitution must not rewrite the shadowed `a`.
6. Multi-parameter cases: `#[case(2, 3 => 5)]`.

---

## 8. Effort, risks, non-goals

- **Effort:** ~150–250 lines (translator substitution + case plumbing + report),
  plus tests. No compiler work, no prover work.
- **Risk 1 (the real one):** cases must never be mistaken for proof. Mitigated by
  the distinct outcome and the `all_proven()` rule.
- **Risk 2:** substitution is a scope-sensitive rewrite; the shadowing test is
  mandatory.
- **Non-goals:** property-based generation (that is `verify_samples` at runtime),
  and proving the general goal from cases (cases do not generalise).

## 9. Sequencing

This is independent of the pending work, and pairs naturally with it:

1. aliases + `Report.warnings` (§ the naming decision) — small;
2. `#[case]` + `#[intent]` — this document;
3. `counsel check` CLI — displays both, with cases in their own section.

Doing 1 first is convenient: `#[intent]` and the case verdicts both need the
report to carry more than a single verdict per function.
