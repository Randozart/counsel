# Should the contract macros live in the core crate?

**Date:** 2026-11-12
**Mode:** PLAN (research-first; no code changed)
**Question (owner):** "Should the macros not be in the core crate?"

## Short answer

**They cannot literally be** — but they *should* be **re-exported** from `counsel`
so `cargo add counsel` is the only dependency a user needs. Keep the separate
proc-macro crate; it is a technical requirement, not a packaging preference.

---

## 1. The hard constraint (why "in the crate" is impossible)

`counsel-macros/Cargo.toml` sets:

```toml
[lib]
proc-macro = true
```

A `proc-macro = true` crate is a separate compilation model: it may export only
macros, and its items live in the macro namespace. It **cannot** be a `mod`
inside `counsel`, and `counsel` cannot export `#[proc_macro_attribute] pub fn`
directly. Every Rust crate that offers attribute macros faces this; the standard
answer is a companion crate plus a re-export.

So the real choice is not "merge or not" but **"does `counsel` re-export them?"**

## 2. What the macros actually are

`counsel-macros/src/lib.rs` — 38 lines total, **zero dependencies**:

```rust
#[proc_macro_attribute]
pub fn pre(_attr: TokenStream, item: TokenStream) -> TokenStream { item }
```

Same for `post` and `case`. Each **expands to the annotated item unchanged**.

They exist for one reason, documented in the crate: rustc rejects unknown
attributes, so a real crate cannot compile `#[pre(...)]` without them. The
*checker* never needs them — `counsel-static` parses the source with `syn` and
reads the attribute tokens directly (`contract_attr(&item_fn.attrs, "pre")`,
`counsel-static/src/translate.rs:43`).

`counsel-macros/tests/accepts.rs` pins the contract: attributes are accepted and
inert (`double(5) == 10`, no runtime behaviour).

## 3. Current wiring (and the gap)

| Crate | Depends on macros? | Re-exports them? |
|---|---|---|
| `counsel` (runtime) | no — `[dependencies]` is empty, no `[features]` | **no** |
| `counsel-static` (checker) | no | no |
| `counsel-macros` | — | — |

`README.md:62` therefore tells users to add **two** crates:

> `counsel-macros` — you want `#[pre]`/`#[post]`/`#[case]` to compile

That is the gap: the one thing a user cannot discover from `cargo add counsel`
is the attribute syntax the whole product is built around.

## 4. Recommendation

**Re-export from `counsel`, behind a default-on `macros` feature.**

```toml
# counsel/Cargo.toml
[dependencies]
counsel-macros = { version = "0.1", path = "counsel-macros", optional = true }

[features]
default = ["macros"]
macros  = ["dep:counsel-macros"]
```

```rust
// counsel/src/lib.rs
#[cfg(feature = "macros")]
pub use counsel_macros::{case, post, pre};
```

Why this shape:

- **One dependency.** `cargo add counsel` then `use counsel::{pre, post, case};`
  — matches how `tokio` re-exports `tokio_macros` and `serde` re-exports
  `serde_derive`.
- **Default-on, not off.** Serde defaults `derive` off because most users only
  deserialize; here the attributes *are* the product's surface, and the crate is
  38 lines with no dependencies, so the build cost is negligible.
- **Opt-out preserved.** `default-features = false` keeps the runtime half free
  of a proc-macro build for anyone who wants it.
- **No new coupling.** `counsel-macros` has no dependencies, so this creates no
  cycle and no version-skew risk beyond the 0.1.x lockstep any companion crate
  has.

### Why not the alternatives

| Alternative | Verdict |
|---|---|
| Literally move the macros into `counsel` | **Impossible** — `proc-macro = true` is crate-level |
| Re-export from `counsel-static` instead | Works, but the crate users type is `counsel`; and the checker is the heavier half (`syn`, `serde_json`) — attributes belong with the light umbrella |
| Re-export `counsel-static` from `counsel` as well | Reject — pulls `syn` + `serde_json` into every consumer |
| Drop attributes for doc-comment contracts (`/// # Pre: …`) | Reject — removes the macro crate but makes contracts stringly-typed and unparseable by rustc |
| Keep two crates, document only | Status quo; works, but the syntax is undiscoverable from `cargo add counsel` |

## 5. The real risk to manage: inertness

The attributes **enforce nothing by themselves**. Enforcement is either:

- `counsel-check` / `counsel-static` (static; reads source, runs Briev's prover), or
- `contract(pre, body, post, x)` (runtime; in `counsel`).

Re-exporting makes the attributes *more* prominent, which raises the chance a
user believes `#[pre]` guards a call at runtime. Two cheap mitigations:

1. Put one line directly under the attribute table: *"inert markers; enforcement
   comes from `counsel-check` in CI (or `contract()` at runtime)"*.
2. Make the re-export's doc comment say the same thing (it shows on docs.rs).

This is also why the re-export must **not** be renamed to something that sounds
enforcing (e.g. `require`).

## 6. Work plan (if approved)

1. `counsel/Cargo.toml`: add the optional `counsel-macros` dependency + `[features]`.
2. `counsel/src/lib.rs`: `pub use` behind `#[cfg(feature = "macros")]`.
3. Test in `counsel/tests/`: `use counsel::{pre, post, case};` on a fn — proves
   the re-export resolves and expands (mirrors `counsel-macros/tests/accepts.rs`).
4. README: collapse the three-crate table to "`cargo add counsel` gives you the
   runtime API **and** the attributes"; keep `counsel-static` separate, and state
   the inertness caveat next to it.
5. `counsel-macros/Cargo.toml`: add `repository`, `homepage`, `documentation`,
   `readme` (the same metadata gap already fixed in `counsel`; `cargo publish`
   warns without them).
6. Publish order becomes: **`counsel-macros` → `counsel-static` → `counsel`**
   (path deps must carry a `version`, so the macros ship first).
7. Gates: `cargo test` (workspace), `cargo publish --dry-run -p counsel-macros`
   then `-p counsel` (each rebuilds in isolation and must pass), praetor on
   changed dirs.

**Estimated size:** small — one manifest field set, one `pub use`, one test, doc
edits. No change to `counsel-static` or to any behaviour.

## 7. Decision requested

Adopt the re-export (recommended), or keep the two-crate story exactly as
documented today?
