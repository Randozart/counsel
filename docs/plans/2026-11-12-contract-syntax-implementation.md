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
