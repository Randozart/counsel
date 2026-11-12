# Roadmap — Counsel as a standalone tool

**Date:** 2026-11-12
**Status:** items 1–4 done, plus C4. C1 (`2b361ed2`, `3c41fe6`), C6+C5
(`860abe10`), T1 (`4574ca8`), T2+T3 (`088e631`), C4 (`984ca4ac`), C2 (`build.rs`
resolver). Remaining: C3 inter-procedural facts.
**Basis:** `2026-11-12-standalone-assessment.md` (readiness + debt inventory).

Order follows the assessment: trust and interface first, then adoption, then depth.

---

## Item 1 — C1: structured diagnostics ✅ done (`2b361ed2`, `3c41fe6`)

**Problem.** `counsel-static` learns verdicts by parsing `brievc`'s stderr prose:

```
error: --verify-bodies:
lies: body can violate its postcondition: a path returns term = 5, ...
warning: --verify-bodies could not prove: scale: could not prove the postcondition on 1 path(s) ...
```

It splits on `:` and prefix-matches English sentences. That is an undocumented
contract: any wording change silently degrades the tool (a miss reads as
"proven").

**Fix (compiler side).** `--verify-report <path>` writes the existing
`BodyReport` as JSON; `--verify-bodies` behaviour is unchanged.

```json
{
  "proven":   ["double"],
  "unproven": [{ "name": "double", "reason": "could not prove the postcondition on 1 path(s)" }],
  "violated": [{ "name": "lies",   "reason": "body can violate its postcondition: a path returns term = 5" }]
}
```

**Fix (tool side).** `counsel-static` prefers the report file and falls back to
text parsing when it is absent (older compiler). The fallback is documented as
legacy and removable once a compiler with the flag is the floor.

**Acceptance**
- `brievc build f.bv --library --verify-bodies --verify-report r.json` writes the
  file; verdicts and reasons round-trip.
- Violations still fail the build (exit 1) and still print human text.
- `counsel-static` produces identical `Report`s from the JSON path; a test
  asserts the JSON path is taken when the compiler supports it.
- briev-lang suite green; counsel suites green.

## Item 2 — C6 + C5: trust and defaults ✅ done (`860abe10`)

- **C6** `--verify-bodies` prints the compiler's identity (version/commit) and
  warns when `brievc freshness` would flag it, so a stale binary cannot quietly
  report older prover behaviour (this cost us a detour).
- **C5** a strict profile (`--strict` or `.s`-profile) enables body verification
  by default, so new code gets contracts proved without knowing the flag.

## Item 3 — T1/T2/T3: make it runnable by a stranger ✅ done (`4574ca8`, `088e631`)

- **T1** proc-macro shim so `#[pre]`/`#[post]`/`#[case]` are accepted by rustc
  (no-op expansion; `counsel-static` still reads the source text).
- **T2** directory/project scanning and `cargo counsel check`.
- **T3** `--format json` for the CLI (consumes the same `Report`).

## Item 4 — C2: packaging ✅ done

Publish `briev-compiler` (or ship prebuilt `brievc` binaries) so `cargo add
counsel` works. Deliberately after the interface settles: the published shape
should not change twice.

**Shipped as the prebuilt-binary route.** `build.rs` resolves the compiler in
order: `COUNSEL_BRIEVC`/`BRIEVC` → pinned release cache → `brievc` on `PATH` →
download the pinned, sha256-verified release tarball. The bundle keeps
`<prefix>/bin/brievc` + `<prefix>/share/briev/`, which the compiler resolves from
its own executable, so no `BRIEV_HOME` plumbing is needed. Cache lives at
`$XDG_CACHE_HOME/counsel` (override `COUNSEL_CACHE_DIR`); `COUNSEL_NO_DOWNLOAD=1`
forbids fetching, and `DOCS_RS` skips the native half so docs build offline.
Publishing `briev-compiler` to crates.io stays deliberately unclaimed (it would
push an LLVM toolchain onto every consumer).

Verified: fresh cache downloads + verifies + builds (3.9 s); warm cache builds
with a dead proxy (3.2 s, no network); explicit `BRIEVC` unchanged (3.2 s); no
compiler and no network yields one message naming all four options.

## Item 5 — C3 + C4: prover depth

- **C3** reason about bodies containing calls (currently skipped wholesale by
  `body_supported`) — needs summaries or inter-procedural facts.
- **C4** wire Z3 into `--verify-bodies` so hard goals are decided rather than
  reported `Unproven`, with the install story documented.

---

## Working rules for this roadmap

- Compiler changes land in `briev-lang` with their own commits and tests; tool
  changes land here.
- Every item updates `docs/ARCHITECTURE.md` / `docs/EXAMPLES.md` in the same
  commit — an outdated doc is a bug.
- No item is "done" until it is verified against a **freshly built** `brievc`
  (`brievc freshness` must say `fresh`).

---

## C2b — the released `brievc` is not relocatable (finding, 2026-11-12)

The v0.1.0 release pipeline works: tag → build → asset, 4m9s, verified. The
**artifact it produces does not work anywhere but its build tree.**

Evidence (same binary, same source file, only the location changes):

| Layout | Result |
|---|---|
| binary in the compiler repo, cwd = that repo | exit 0, verdicts correct |
| binary + `lib/` + `config/` copied to another dir | `type mismatch: expected Float for term value vs declared return type, found Int` |
| same, with `BRIEV_CONFIG_DIR` pointing at the copied config | same error |
| cwd = the copied package dir (binary beside `lib/`) | same error |
| cwd with a `lib` symlink beside the binary | same error |

Two distinct defects, in order of what they hide:

1. **Absolute build-time paths.** The published binary looks for
   `/lib/std/cli.bv`, `/imports/…`, `/std/…` — i.e. roots that only make sense
   on the machine that built it. `--stdlib-path` overrides the module root, but
   that is a flag users must discover and a path they must supply.
2. **Relocation changes typing.** With `lib/` adjacent (which the resolver does
   consult, `src/import_resolver.rs:423`) the module error disappears, yet
   `Int` degrades to `Float` in the contract: the prelude's type metadata is not
   being loaded from the relocated root. So the compiler depends on its source
   tree for more than module resolution.

**Conclusion.** This is not a workflow bug to patch — it is a compiler property:
`brievc` assumes it lives in its build tree. Patching the workflow to ship a
tarball would ship a binary that still mis-types.

**Proposed fix (compiler).**

- Resolve **one** resource root from `std::env::current_exe()` and use it for
  `lib/`, `config/` and `glue/` — not a mix of cwd-relative, source-relative and
  build-time-absolute paths.
- Allow an override (`BRIEV_ROOT`) for unusual layouts.
- Add an acceptance test that copies `brievc` + the resource tree to a temp dir,
  compiles a contract from an unrelated cwd, and asserts the same verdicts as
  in-tree — this test would have caught both defects.

Until then the release asset carries a caveat and the README says to build the
compiler from source.

### Update — further isolation (same day)

The first table suggested a single relocation defect. Supplying more of the tree
shows it is a *set* of independent path assumptions: each fix exposes the next.

| Package contents | Failure |
|---|---|
| `brievc` alone | `Cannot find module 'std/cli.bv'` (searches `/lib`, `/imports`, `/std`) |
| + `lib/` `config/` `glue/` | `type mismatch: expected Float for term value vs declared return type, found Int` |
| + `std/` `imports/` `spec/` … | `cannot read 'std/c/coll_arena.c'` (a C source read from a relative path) |

Two useful negatives: a **byte-identical** copy of `lib/` (0 symlinks, same file
list, same size) reproduces the typing failure, so it is not a copy artifact; and
a fully self-contained package with the source *inside* it still fails, so it is
not the source location either.

One apparent success earlier (`--stdlib-path` + repo tree) turned out to be
confounded: `/tmp` still held symlinks from an earlier experiment, so the
compiler was reading the real repo tree rather than the copy. With those removed
the failure returns — worth recording, because it is exactly the kind of
environment leak that makes a diagnosis look solved.

**Conclusion, sharper.** `brievc` resolves modules, type metadata and even C
sources from several CWD- and source-relative roots plus build-time-absolute
fallbacks. No packaging arrangement can satisfy that reliably. The fix remains
the one proposed above: a single resource root, resolved from
`current_exe()` with a `BRIEV_ROOT` override, plus an acceptance test that runs
the shipped binary from a temp directory and compares verdicts with the in-tree
run.

### Handoff — what is established, and the one open question

Established by experiment (all recorded above):

1. The release **pipeline** works; the **artifact** does not.
2. Relocation fails in a cascade — supplying more of the tree reveals the next
   missing path — so it is a *set* of path assumptions, not one bug.
3. The typing failure (`Int` → `Float`) is **independent of the stdlib root**:
   a packaged layout with `--stdlib-path` pointing at its own `lib/`, run from a
   foreign cwd, still fails. It is also not a copy artifact (byte-identical
   `lib/`) and not the source location (source inside the package fails too).

**Not established: why `Int` degrades to `Float`.** Everything else is
path-shaped; this one is not, so it needs instrumentation rather than more
packaging guesses. The concrete next probe:

- Locate the emitter of `expected Float for term value vs declared return type,
  found Int` (the string is assembled with `format!`, so grep for `term value`
  finds only its doc-comment sibling in `import_resolver.rs:298`; search the
  typechecker for `expected {}` + `term`).
- Log the type that `term` is bound to at contract elaboration
  (`typechecker/mod.rs`, where `term` is inserted with
  `output_type_to_type(&OutputType::Single(ty))`), and whether the fundamentals
  module (`lib/std/types/bootstrap.bv`) was spliced in that run. `--emit-beast
  parse.after` should show whether the prelude actually landed.

Once that is known, the fix is the one proposed above (single resource root from
`current_exe`, `BRIEV_ROOT` override) **plus** whatever the instrumentation
shows, and the acceptance test is the part that must not be skipped:

> copy `brievc` + its resource tree to a temp dir, compile a contract from an
> unrelated cwd, and assert the verdicts equal the in-tree run.

That test would have caught all three cascade failures *and* the typing
degradation; without it, "packaging" stays a hope rather than a check.

### Correction — the typing failure is not independent of paths

The handoff above says the `Int` → `Float` degradation is independent of the
stdlib root. That was too strong, and the code says so:

- `src/parser/types.rs` maps `"Int" => Type::int()`, so `-> Int` parses
  correctly; the declared type cannot be Float on its own.
- Re-reading test N: the package that included the **top-level `std/`** got
  *past* typing entirely and failed later, at `cannot read 'std/c/coll_arena.c'`.

So the Float came from a run with no prelude, not from a type-mapping bug. What
`--stdlib-path` failed to supply was the top-level `std/` (the prelude,
`std/core.bv`), which is a different root from the module library.

**Revised resource set:** `lib/`, `config/`, `glue/`, `std/` — four roots, not
one, which is precisely why the fix should collapse them into a single
`current_exe`-derived root.

**Revised remaining defect:** C-source resolution. `lib/std/c/coll_arena.c`
exists, yet the packaged run asked for `std/c/coll_arena.c`, so the C path is
built from a root that is *not* the resolved stdlib. Next probe: find where that
path is constructed (grep for `std/c` rather than the filename — the earlier
`coll_arena.c` grep in `src/` found nothing, so the path is assembled) and make
it hang off the same single root.

### Resolution — it was `plugins/`, and the typing really was path-dependent

Both earlier hypotheses were wrong in an instructive way. The failure was not a
type-mapping bug (`parser/types.rs` maps `Int` correctly) and not a missing
`std/` root. It was `plugins/`:

`discover_system_plugins` checked `./plugins`, fell back to
`<resource_root>/plugins`, and **silently continued** when neither existed.
`plugins/parsed/prelude-native.bv` is the prelude, so an incomplete resource
tree removed the fundamentals and the checker settled on `Float` for an `Int`
contract. Because the working directory came first, the same tree typed
differently depending on where you stood — the repo supplied `./plugins`, a
package could not.

My "independent of the stdlib root" note was true and misleading: independent of
the *stdlib* root, entirely dependent on the *plugin* root. The lesson worth
keeping is the diagnostic one: `BRIEV_HOME=<original repo>` making the package
compile from its own cwd narrowed the variable to *root resolution*, and a
bisect over the repo's 612 root entries then named `plugins/` ("needed alone:
YES"). The first bisect had inverted logic and converged on a random `.bc` with
"needed alone: no" — that contradiction is what exposed the bisect, not the
compiler.

Shipped:

| Repo | Commit | Change |
|---|---|---|
| briev-lang | `2172bbc6` | resource root wins; `./plugins` dev-only fallback; missing/empty plugin dir is a hard error; `Result` propagated through `build_plugin_manager` |
| briev-lang | `22347f76` | `tests/relocatability.rs` — compiles from an unrelated cwd; a missing prelude fails loudly and never as a type mismatch |
| counsel | `6530736` | tarball `<prefix>/bin/brievc` + `<prefix>/share/briev/{lib,config,glue,std,spec,plugins}`, a relocatability gate before publishing, README install rewritten |

`v0.1.0` was re-cut so the published artifact carries the fix. C2c is closed;
what remains on the roadmap is prover depth (C3 call-containing bodies, C4 Z3
wiring), not packaging.
