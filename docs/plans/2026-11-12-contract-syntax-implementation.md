# Implementation — Counsel contract syntax (`pre`, `post`, `case`)

**Date:** 2026-11-12
**Status:** in progress.
**Decision record:** `2026-11-12-contract-attribute-naming.md` §8 (decided) and
`2026-11-12-intended-cases.md` (mechanism).
**Decided surface:** exactly three attributes — `#[pre]`, `#[post]`, `#[case]`.
No `requires`/`ensures` aliases, no `#[intent]`.

---

## Step 1 — rename + safety guards (this commit)

**What changes**

| Item | Before | After |
|---|---|---|
| Precondition attribute | `#[requires(expr)]` | `#[pre(expr)]` |
| Postcondition attribute | `#[ensures(expr)]` | `#[post(expr)]` |
| Duplicate detection | none (first match silently wins) | hard error |
| Unknown/misspelt attribute | silently ignored → function skipped, no contract | warning naming the accepted spellings |
| `Report` | `verdicts` only | `verdicts` + `warnings` |

**Duplicate guard.** `contract_attr` returns on the first match, so
`#[pre(a)] #[pre(b)]` would drop the second silently. It must scan every
attribute and error when the same slot appears twice:

```
more than one `#[pre(...)]` on this function — keep one
```

**Typo warning.** Today a misspelt attribute means *no contract, no
verification, no warning* — the function is skipped. After the rename the two
most likely mistakes are the old names, so both are called out explicitly:

| Written | Message |
|---|---|
| `#[requires(…)]` | renamed to `#[pre(…)]` |
| `#[ensures(…)]` | renamed to `#[post(…)]` |
| near-miss (`#[pr]`, `#[postt]`, `#[casee]`) | "did you mean …" |
| anything else (`#[inline]`, `#[test]`, `#[derive]`) | ignored silently |

**Acceptance**

- `#[pre]` / `#[post]` produce the same Briev `[pre][post]` as before.
- Two `#[pre]`s → `Error` naming the duplicate.
- `#[requires]` → warning, function still verified via a correctly-named
  attribute if present, else skipped **with** a warning.
- `Report.warnings` surfaced by `check_source`.
- Existing suite green (22 tests + 4 doctests) after the sweep.

## Step 2 — `#[case(inputs => expected)]`

Scope-aware literal substitution in the translator; per-case Briev `defn`;
`ProvenForCase` / `CaseFailed`; `all_proven()` counts symbolic proofs only.
Six tests, the mandatory one being shadowing (`let a = 1;` must not be
rewritten by a case substituting `a`).

## Step 3 — follow-ups

- Stale status in `2026-11-11-briev-share.md`.
- Collapse `verify` (Rust closures) and `verify_samples` (Briev) to one
  canonical path.
- `counsel check` CLI + CI.

## Risk

Breaking for any existing user — mitigated by the fact that the crate is 0.1.0,
no proc-macro is shipped, and the old names now warn with the replacement.

---

## Step 2 — delivered (2026-11-12), with one deliberate deviation

**Shipped syntax:** `#[case(5)]` and `#[case((2, 3))]` — concrete *inputs* only.

**Not shipped:** the `=> expected` right-hand side. Reason: honouring it requires
evaluating the body at those inputs, which the static path does not do. Shipping
an `expected` value that is parsed but never checked would be exactly the
silent weakness this crate exists to avoid — a case that "passes" while its
stated expectation was never compared. The expectation is therefore taken from
the function's own `#[post]`, which *is* proved at that input, and there is no
second place to get it wrong.

If `=> expected` is wanted later, it becomes a cross-check with real meaning
once the body can be evaluated (or by emitting a second concrete goal
`[term == <expected>]`, which the prover already decides).

**Mechanism as built**

- `case_values_for` reads every `#[case(...)]`, requiring one literal per
  parameter (wrong arity is `Error::Contract`).
- `instantiate_case` re-emits the already-translated defn as `() -> Int` with
  parameter identifiers substituted in *both* the contract and the body
  (`defn double__case0() [5 >= 0][term == 5 + 5] -> Int { term 5 + 5; };`).
- Substitution is textual, so `instantiate_case` **refuses** a function whose
  body rebinds a parameter (`let a = ...`) rather than mis-substituting.
- `Outcome::ProvenForCase` marks a case verdict; `Report::all_proven()` counts
  symbolic `Proven` only, so cases can never turn the report green.

**Tests:** 4 new unit tests (substituted emission, multi-arg left-to-right
mapping, shadowing refused, arity refused); suite 21 unit + 8 integration green.
