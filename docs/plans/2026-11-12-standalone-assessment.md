# Counsel as a standalone tool — readiness and the compiler debt behind it

**Date:** 2026-11-12
**Status:** assessment (no code changed).

---

## 1. What exists today

| Piece | State |
|---|---|
| `counsel` (runtime) | Briev-authored predicates + `run_checked` + `verify_samples` compiled to a static lib and linked in; 14 public functions |
| `counsel-static` (static) | Rust-subset translator + discharge through `brievc --verify-bodies`; `check_source` / `check_file` |
| `counsel-check` CLI | one file in, verdicts out; exit 0 / 1 / 2 |
| Docs | `ARCHITECTURE.md`, `EXAMPLES.md` (incl. cases), two design records, one implementation plan |
| Tests | counsel 22; counsel-static 21 unit + 9 integration; briev-lang 3699 |
| CI | workflow written and YAML-validated; **never executed on a runner** |

The engine is real: contracts are discharged by Briev's own body prover, and
`Proven` / `ProvenForCase` / `Unproven` / `Violated` are kept distinct.

## 2. Standalone-tool readiness

| Requirement for "someone else can use it" | Status | Gap |
|---|---|---|
| `cargo add counsel` works | ✗ | `build.rs` shells to `brievc`, which is not published anywhere |
| `cargo add counsel-static` works | ~ | compiles, but every useful call needs `brievc` at check time |
| Contracts can live in a real crate | ✗ | `#[pre]`/`#[post]`/`#[case]` are **unknown attributes to rustc** — no proc-macro exists, so a compiled crate rejects them |
| Check a whole project | ✗ | CLI takes exactly one path (`args().nth(1)`); no directory walk, no cargo integration |
| Machine-readable output | ✗ | human lines only; no JSON |
| Runs in CI for a stranger | ✗ | workflow unexecuted; needs a `briev-lang` build first |
| Documented verdict semantics | ✓ | `EXAMPLES.md` §9, `ARCHITECTURE.md` §4 |

**Verdict:** a working library + prototype CLI with a genuine prover behind it.
Not yet adoptable: the two hard blockers are *installability* and *attributes
that rustc accepts*.

## 3. Compiler work — what was fixed, what remains

Fixed during this workstream (all upstream, all tested):

| Fix | Commit |
|---|---|
| Exported defns accept flat C function pointers (trampolines) | `43519b01` |
| Forwarding a callback parameter converts the handle back to `ptr` | `0ea0fe6e` |
| No dangling `ret` when every path terminates inside guards | `cf17aace` |
| A point fact pins a value, so `[a == 5]` folds arithmetic | `a2bbaba4` |

Remaining, ordered by how much each unblocks:

| # | Gap | Blocks | Effort |
|---|---|---|---|
| C1 | **Structured diagnostics**: `counsel-static` parses stderr text (`error: --verify-bodies:`, `warning: --verify-bodies could not prove: <name>: …`, `split_once(':')`). `brievc` has no `--json`/`--report` flag. | Any robust tool integration; the text is an undocumented contract | **S** — serialise the existing `BodyReport` |
| C2 | **Packaging**: `brievc` is not installable (no crates.io publish, no prebuilt binary) | `cargo add counsel`; CI for strangers | **M** — publish `briev-compiler` or ship binaries |
| C3 | **Prover coverage**: bodies containing calls are skipped outright (`body_supported`), so a contract on a function that calls a helper can never be proven | Most real functions | **L** — needs inter-procedural reasoning or summary facts |
| C4 | **Z3 is optional and external**; without it only heuristics + interval bounds run | Undecided goals that need real SMT | **M** — wire it, or document the gap honestly |
| C5 | `--verify-bodies` is opt-in (`verify_bodies: false` default) | Users must know the flag exists | **S** — a profile (`--strict`) or default-on for new code |
| C6 | Freshness trap: a stale `brievc` silently behaves like an older prover | Trust in every result (cost us today) | **S** — have the gate print its version/commit |

## 4. Tool work (counsel-side) that does not need the compiler

| # | Item | Effort |
|---|---|---|
| T1 | **Proc-macro shim** so `#[pre]`/`#[post]`/`#[case]` are accepted by rustc (no-op expansion; `counsel-static` reads the real text) | M |
| T2 | Directory/project scanning + `cargo counsel check` | S |
| T3 | `--format json` for the CLI | S |
| T4 | Multi-file contract surface (today: single file, single function set) | S |

## 5. Recommended order

1. **C6 + C5** (small, trust and defaults) — then **C1** (structured report), because every later tool feature rests on a stable interface instead of stderr prose.
2. **T1 + T2 + T3** — turns the prototype into something a Rust developer can actually run on their crate.
3. **C2** — packaging, once the interface is stable, so the published shape does not change twice.
4. **C3 + C4** — prover depth; the biggest wins, and the ones that make "lighter than Kani" defensible on more than straight-line integer code.

## 6. Honest summary

- **Counsel is real**: a small, honest verifier whose engine is the language's own prover, with three fixed compiler defects and one prover improvement delivered upstream to make it possible.
- **It is not yet a tool**: no installable compiler, no rustc-accepted attributes, no project-wide run.
- **Compiler debt that matters**: one structural item (structured diagnostics), one packaging item, two depth items (call-containing bodies, SMT). The rest is small polish.
