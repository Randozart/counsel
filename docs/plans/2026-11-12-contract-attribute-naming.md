# Contract attribute naming — `requires`/`ensures` vs `pre`/`post` vs the alternatives

**Date:** 2026-11-12
**Status:** decision needed (no code changed).
**Question (owner):** should we use `c-pre`/`c-post` instead of `expected`/`required`,
or just `pre`/`post`?

---

## 1. Correction of the premise

No `expected`/`required` naming ever shipped. `counsel-static` parses:

```rust
#[requires(a >= 0)]
#[ensures(term == a + a)]
fn double(a: i64) -> i64 { a + a }
```

— `counsel-static/src/translate.rs:41-42` (`contract_attr(&attrs, "requires")`,
`contract_attr(&attrs, "ensures")`).

So this is **not** a migration of an established name; it is a free choice made
once, before any real users and before a proc-macro exists. That is the cheapest
moment to decide, and also the only moment where the answer costs nothing.

---

## 2. Hard constraint: attribute names are Rust identifiers

An attribute's name is a `SimplePath` — path segments that are identifiers.
`-` is a separate token, so **`#[c-pre]` cannot parse** (rustc: "expected
identifier"; `syn` fails likewise before we ever see it). `#[c_pre]` is legal.

Consequence: the `c-pre` / `c-post` form is not available at all. `c_pre` /
`c_post` are available. (Worth a five-second probe at implementation time to
confirm the hyphen rejection in the exact syn version.)

---

## 3. Candidates

| Candidate | Valid | Reads as | Risk |
|---|---|---|---|
| `#[requires]` / `#[ensures]` | yes | a sentence about the contract | none known |
| `#[pre]` / `#[post]` | yes | Briev's own `[pre][post]` vocabulary, 1:1 | very generic names |
| `#[required]` / `#[expected]` | yes | adjectives, no object | ambiguity + collisions (below) |
| `#[c_pre]` / `#[c_post]` | yes | "C pre?" | collides with C-ABI language |
| `#[c-pre]` / `#[c-post]` | **no** | — | not parseable |

### Why `expected`/`required` are the weakest of the valid options

- **`expected` collides twice.** Rust test vocabulary already uses it
  (`#[should_panic(expected = "...")]`, `assert_eq!(a, b, "expected ...")`), and
  **Briev itself has an `expect <bool>;` statement** — a test assertion
  (`Statement::Expect` in the AST). Naming a postcondition "expected" would sit
  next to Briev's `expect` meaning something else.
- **`required` is ambiguous.** It reads as "this function is required /
  mandatory", not "this must hold to call". Preconditions are about *permission*;
  "required" states obligation.
- **Asymmetry.** Both are past participles, but `expected` names the *output*
  while `required` names the *input constraint* — the pair does not echo.

### Why `c_pre`/`c_post` should be rejected in this codebase specifically

We spent this entire workstream on the **C ABI boundary**: `extern "C"`, flat
`int64_t (*)(int64_t)` callbacks, `__c_cb0` trampolines, `%state` handles. In
this repo, a leading `c_` reads as *C ABI*, not *contract*. `#[c_pre]` would be
actively misleading to the next reader — a violation of the project rule that
special treatment must be disclosed, not obscured.

### `pre`/`post` — the legitimate contender

- Maps **1:1** onto what the translator emits: `#[pre]` → `[pre]`, `#[post]` →
  `[post]`. Zero mental translation for anyone who knows Briev.
- Shortest typing, and the bracket form is already the language's TS term.
- Cost: the names are generic, and in a Rust file they read as jargon until the
  reader sees the doc. (Not a hard problem — `#[post]` next to a `term` name is
  self-explanatory.)

### `requires`/`ensures` — the ecosystem default

This is the de-facto vocabulary for exactly this feature, in the tools Counsel
is explicitly positioned against and alongside:

- **Kani**: `#[kani::requires]`, `#[kani::ensures]`
- **Creusot**: `#[requires]`, `#[ensures]`
- **Prusti**: `#[requires]`, `#[ensures]`
- **Dafny**: `requires` / `ensures`; **Eiffel**: `require` / `ensure`;
  **JML**: `requires` / `ensures`

Because the crate's pitch is "lighter than Kani", matching Kani's *contract
vocabulary* while differing in *weight* is a coherent story: same words, much
less machinery. `#[requires]`/`#[ensures]` also read as complete sentences
("requires a ≥ 0", "ensures term == a + a"), which is what the house style asks
for in diagnostics.

---

## 4. Rename surface (if we change)

Current usage, by file:

| File | Mentions |
|---|---:|
| `docs/EXAMPLES.md` | 13 |
| `counsel-static/src/translate.rs` | 12 |
| `counsel-static/tests/valve.rs` | 8 |
| `counsel-static/src/lib.rs` | 6 |
| `docs/ARCHITECTURE.md` | 4 |
| `README.md` | 3 |
| `docs/plans/*` (2 files) | ~4 |

Code change is two literals plus the doc comment; the rest is prose. **Breaking
for any existing user** — but the crate is 0.1.0 with no proc-macro shipped, so
today is the cheapest possible moment.

---

## 5. Recommendation

> **Superseded by §8 (owner decision, 2026-11-12).** The owner chose `pre` /
> `post` as Counsel's own syntax, so the ecosystem-familiarity argument below
> lost to the 1:1 Briev mapping. Kept for the record.

**Keep `requires` / `ensures` as canonical**, and state the Briev mapping
explicitly in the docs (`#[requires] → [pre]`, `#[ensures] → [post]`) — which
the translation table already does.

Rationale, in priority order:
1. It is the vocabulary of the tools we compare to (Kani/Creusot/Prusti/Dafny),
   so "lighter than Kani" reads as a weight claim, not a dialect.
2. It is unambiguous where the alternatives are not (`expected` collides with
   test vocabulary *and* Briev's `expect`; `required` inverts the meaning).
3. It survives the proc-macro future unchanged.

**If the priority is instead Briev vocabulary symmetry**, `#[pre]` / `#[post]`
is the correct choice — it is honest, short, and maps 1:1 — and is strictly
better than every other alternative in this document.

**Do not** adopt `c_pre`/`c_post` (C-ABI confusion) or `c-pre`/`c-post`
(unparseable).

## 6. Decision needed

- **Option A:** keep `requires`/`ensures`; add a one-line mapping note to
  README + ARCHITECTURE; no code change.
- **Option B:** rename to `pre`/`post`; touch the two literals, tests, and the
  8 files listed above; document the same mapping.
- **Option C (see §7):** keep `requires`/`ensures` canonical **and** accept
  `pre`/`post` as first-class aliases, rejecting ambiguous use.

---

## 7. Aliasing (owner asked: "can we alias to use both?")

**Yes — it is a small change — but it must not be a silent one.**

### What the code does today

`counsel-static/src/translate.rs`:

```rust
let pre  = contract_attr(&item_fn.attrs, "requires")?;
let post = contract_attr(&item_fn.attrs, "ensures")?;
if pre.is_none() && post.is_none() { continue; }      // silent skip

fn contract_attr(attrs, name) -> Result<Option<String>, Error> {
    for attr in attrs {
        if !attr.path().is_ident(name) { continue; }
        let expr = attr.parse_args()?;                // message embeds `#[{name}(...)]`
        return Ok(Some(translate_expr(&expr, true)?)); // FIRST match wins
    }
    Ok(None)
}
```

### Two hazards aliasing would amplify

1. **First-match-wins is an implicit precedence rule.** If a function carries
   both `#[requires(a > 0)]` and `#[pre(b > 0)]`, the loop silently takes
   whichever appears first — order-dependent behaviour that is invisible in the
   source. Aliasing makes this mistake *likely* (copy-paste of both spellings),
   so it must become a hard error naming both spellings.
2. **A misspelt attribute is silently ignored today.** `#[requiers(a > 0)]`
   matches nothing, so both slots are `None`, and the function is skipped with
   **no contract, no verification, no warning**. Aliasing doubles the names to
   misspell, so adding aliases plausibly *without* a diagnostic makes the tool
   quietly weaker — the exact opposite of the crate's promise that `Unproven`
   is never treated as a pass.

### Proposed rules

| Rule | Behaviour |
|---|---|
| Canonical names | `#[requires]` / `#[ensures]` — used in docs, examples, messages |
| Aliases | `#[pre]` / `#[post]` — identical meaning, identical generated Briev |
| Duplicate for one slot | **hard error**: "both `#[requires]` and `#[pre]` are present; keep one" |
| Error text | quotes the spelling the user actually wrote (not the canonical one) |
| Typos | warn, listing accepted names: `#[requiers]` → "did you mean `#[requires]` or `#[pre]`?" |
| Contracts are per-slot optional | `#[pre]` alone, or `#[post]` alone, stay valid |

### Implementation sketch

- `contract_attr(attrs, &["requires", "pre"]) -> Result<Option<(String, String)>, Error>`
  returning the *matched spelling* alongside the translated expression; error if
  a second spelling for the same slot is present.
- A small `KNOWN = ["requires", "pre", "ensures", "post"]` table drives both the
  duplicate check and the typo hint (edit distance 1–2).
- Typos need a warning channel: extend `Report` with `warnings: Vec<String>`
  (it currently carries only `verdicts`); the CLI/tests surface it.

### Cost

- ~15–25 lines in `translate.rs`; `Report` gains a `warnings` field.
- Tests: alias works; canonical + alias mixed for *different* slots works;
  duplicate for one slot errors; typo warns; docs unchanged in meaning.
- Permanent surface: two vocabularies to document, test, and — if a proc-macro
  ever ships — accept in the macro too. Accepted deliberately, because the
  canonical name keeps Kani-ecosystem familiarity while `pre`/`post` keeps the
  1:1 Briev mapping available.

### Recommendation

**Take Option C**: canonical `requires`/`ensures`, aliases `pre`/`post`,
duplicates rejected, typos warned. It removes the "which spelling do I have to
remember?" question without ever letting two spellings mean two different
things, and the typo warning closes a pre-existing silent-skip hole that
predates this discussion.

---

## 8. Final contract surface — DECIDED (2026-11-12)

**Counsel has its own contract syntax: `pre`, `post`, `case`.**

| Attribute | Slot | Checked by | Notes |
|---|---|---|---|
| `#[pre(expr)]` | precondition | prover, symbolic | at most one per function |
| `#[post(expr)]` | postcondition | prover, symbolic | at most one per function |
| `#[case(inputs => expected)]` | examples | prover, concrete instantiation | repeatable |

`term` remains the return-value name inside postconditions (Briev's own
placeholder), so the mapping to the language is direct: `#[pre]` → `[pre]`,
`#[post]` → `[post]`.

### Why this over `requires`/`ensures`

- **1:1 with Briev.** No mental translation between the Rust surface and the
  emitted `[pre][post]`; the crate's vocabulary matches its engine's.
- **Own identity.** Counsel is not a Kani clone with fewer flags; sharing its
  attribute names implied a compatibility it does not claim.
- **Least surface.** Dropping aliases removes the cross-spelling duplicate rule,
  the matched-spelling bookkeeping, and the "which one is canonical?" question
  from every doc and error message.

### Rules that survive

1. **At most one `#[pre]` and one `#[post]`** per function. A repeated
   `#[pre]` is a **hard error** — `contract_attr` returns on first match, so
   without this guard the second would be silently ignored.
2. **Contracts stay independently optional** (`#[pre]` alone, `#[post]` alone).
3. **Typos warn**, listing the accepted names — closing the existing hole where
   a misspelt attribute means "no contract, no verification, no warning".
4. **A passing case is not a proof.** `Proven` (symbolic, every path) is
   distinct from `ProvenForCase` (that input only); `Report::all_proven()`
   counts symbolic proofs only.
5. **Multi-argument cases** read left-to-right against the parameter list:
   `#[case(2, 3 => 5)]` for `fn add(a: i64, b: i64) -> i64`.
6. **No aliases, no `#[intent]`.** Three attributes total. Prose that is never
   checked does not belong in a crate whose promise is "unproven is never a
   pass".

### Implementation consequences

- `counsel-static/src/translate.rs`: two string literals (`"pre"`, `"post"`)
  replace `"requires"`/`"ensures"` — `contract_attr` keeps its current shape.
- Add a same-name duplicate guard and a typo warning (`Report.warnings`).
- Docs/tests sweep: 8 files, ~46 mentions (`EXAMPLES.md` 13, `translate.rs` 12,
  `valve.rs` 8, `lib.rs` 6, `ARCHITECTURE.md` 4, `README.md` 3, 2 plan docs).
- Published crate at 0.1.0 with no proc-macro: this is the cheapest moment to
  break the attribute names, and effectively free.
