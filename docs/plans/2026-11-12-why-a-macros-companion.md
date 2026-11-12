# Why Counsel still has a `counsel-macros` crate

**Date:** 2026-11-12
**Mode:** PLAN (research-first; no code changed)
**Question (owner):** "Why still a separate macros crate?"

## The answer in one line

Because **rustc will not let a normal library crate export attribute macros** —
this is a language constraint, not leftover packaging. Users still add exactly
one dependency: `counsel`.

## The constraint, precisely

`counsel-macros/Cargo.toml`:

```toml
[lib]
proc-macro = true
```

A `proc-macro = true` crate is a **different compilation model**:

- it may export **only** macro-namespace items (`#[proc_macro]`,
  `#[proc_macro_attribute]`, `#[proc_macro_derive]`);
- it **cannot** export normal items (types, functions, `const`s);
- it **cannot** be a `mod` inside another crate — the flag is crate-level.

And the converse: `counsel` cannot write `#[proc_macro_attribute] pub fn pre`
itself, because it produces a normal library (it has `pub fn contract`,
`pub enum Verdict`, …) and a crate cannot be both.

So "put the macros in the core crate" is not a packaging preference we can
decide — it is not expressible in Rust. What *is* decidable is whether the core
crate **re-exports** them, which `counsel` now does:

```rust
#[cfg(feature = "macros")]
pub use counsel_macros::{case, post, pre};
```

That is why `use counsel::{pre, post}` works and why `cargo add counsel` is the
whole story for a user. The companion crate is an implementation detail, in the
same way `serde` ships `serde_derive` and `tokio` ships `tokio-macros`.

## What it actually costs

| Cost | Measured / stated |
|---|---|
| Extra dependency for users | **none** — `counsel` re-exports; you never add it |
| Extra build unit | **0.85 s** to compile the macros crate (from the publish dry-run) |
| Extra publish step | yes — `counsel-macros` first, then `counsel` (documented in the roadmap) |
| Version lockstep | yes — both at `0.1.x` |
| Repo layout | one extra directory, already a workspace member |

## If the objection is "one crate, really"

There are exactly three ways to have a single crate, and two of them give up
real attributes. All three were checked against the code.

### Option A — keep attributes (current)

`contract_attr` reads a genuine attribute:

```rust
if !attr.path().is_ident(name) { continue; }
let expr: syn::Expr = attr.parse_args()?;
```

- **Pro:** best ergonomics; rust-analyzer and editors validate the expression;
  typo'd names surface as "unknown attribute" at compile time; the product's
  docs, fixtures and 164 `#[pre]/#[post]/#[case]` usages already speak it.
- **Con:** one companion crate + publish order.

### Option B — doc-comment contracts (no macro crate)

`/// # Pre: a >= 0` is the built-in `#[doc]` attribute, so rustc accepts it with
no macro crate at all; `syn` exposes it as `Attribute { path: "doc" }`.

- **Pro:** genuinely one crate, zero proc-macro.
- **Con:** contracts become **strings** parsed out of prose. No editor checking,
  no compile-time typo detection, silent no-ops when the format drifts, and
  rustdoc renders the contract as documentation (a feature or a leak,
  depending on taste). The checker would need a hand-rolled parser and its own
  error surface for malformed contracts.

### Option C — `macro_rules!` wrapper (no proc-macro)

`counsel::contracts! { pre(a >= 0); post(term >= 0); fn f(...) }`. Tokens inside
a `macro_rules!` invocation never reach attribute resolution, so no proc-macro
is required.

- **Pro:** one crate, no proc-macro.
- **Con:** the naive form (putting `#[pre]` *inside* the macro body) **breaks**:
  `macro_rules!` can capture all attributes with `$(#[$m:meta])*` but cannot
  *selectively* drop one, so it must either forward `#[pre]` to rustc (error) or
  strip every attribute — silently deleting doc comments and `#[inline]`. The
  workable shape makes contracts macro arguments, i.e. a different syntax from
  today's, and `check_source` would have to parse `Item::Macro` token streams
  instead of `Item::Fn` (currently `translate.rs:39-40` matches `Item::Fn` only).

### Option D — nest the directory (cosmetic only)

If the objection is really "the repo looks like two crates", `counsel-macros/`
can move under the main crate (e.g. `macros/`). The published tarball is already
clean: the `counsel-0.1.0` package built earlier contains **zero**
`counsel-macros` files, because cargo excludes nested packages.

- **Pro:** one top-level directory.
- **Con:** does not change the Rust constraint, the publish order, or the second
  build unit; nested packages confuse some tooling and path deps read oddly.

## Recommendation

**Keep Option A.** The companion crate is not user-visible, and the two
alternatives that remove it trade attribute ergonomics for stringly-typed
contracts or a worse macro shape. The honest summary is: *one dependency for
users, two crates in the repo, because Rust requires it.*

If a visual single-crate repo matters (Option D), that is a five-minute change
with no effect on publishing — worth doing only if it bothers you.

## What I would change instead (higher value)

1. **Make the publish order painless**: a short `scripts/publish.sh` that
   publishes `counsel-macros` then `counsel`, so the ordering rule cannot be
   forgotten.
2. **Say it in the README** (one line): "the attribute crate is an
   implementation detail; you add `counsel` only" — so the next person does not
   ask this question.
3. Leave the attributes alone: they are the product's surface, and 164 usages
   already depend on them.
