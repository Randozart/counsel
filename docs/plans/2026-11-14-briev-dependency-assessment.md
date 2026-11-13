# Does Counsel actually need Briev?

**Status:** implemented (2026-11-14). The runtime half is pure Rust — `build.rs`,
`bridge/counsel.bv` and `src/bindings.rs` are deleted (counsel `e7c6194`) — so the
default feature set needs no Briev at all. Briev is resolved only at check time
for `--features static`, by a sha256-verified fetch (counsel `f0afd92`).
**Question (Randy):** *"In using Briev, are we actually getting anything, or can we
pre-compile a binary to embed? Is Briev actually helpful here? We don't need Briev
code in the crate to do static proof, no?"*

Three questions are tangled together here, and they have **different answers**:

1. Can we pre-compile and embed a binary? → *Delivery. Mostly no, and it wouldn't
   answer the real question.*
2. Do we need Briev code in the crate for static proof? → *We already don't.*
3. Is Briev actually helping? → *Yes for the prover, weakly for the runtime half,
   and the current pain is not where you'd guess.*

---

## 1. Where Briev actually sits today

### The static half (`--features static`)

```
Rust source ──(syn)──> Counsil translator ──> Briev SOURCE TEXT ──> brievc --verify-bodies ──> verdicts
```

- `counsel/Cargo.toml` dependencies: `counsel-macros`, `serde_json`, `syn`. **There
  is no Briev dependency.** The crate contains **zero Briev code** on this path.
- Briev is a *downloaded binary* (`build.rs` → cache → PATH → sha256-verified
  tarball), invoked as a subprocess at check time.
- What that binary supplies is not trivial: `verify_bodies.rs` **1,543 lines** (path
  walker, guard forking, interval bounds, substitution, inter-procedural
  summaries) + `smt.rs` **345 lines** (SMT-LIB encoding, Z3 escalation).

**So "we don't need Briev code in the crate to do static proof" is already true.**
What we need is a *prover*, and today that prover is someone else's binary.

### The runtime half (default features)

- `bridge/counsel.bv` — **167 lines**, compiled by `build.rs` at **build time** into
  a native C library; `src/bindings.rs` is generated from it and checked in.
- It exports `add`, `within`, `step_up`, `run_checked`, `verify_samples`.
- The file's own comment records that the interesting design was **abandoned**: the
  cross-ABI function-pointer path SIGSEGVs because Briev's LLVM backend lowers a
  fn-ptr parameter to a closure/self convention while the C binding generator
  advertises a flat `int64_t (*)(int64_t)`.

**This is the only place the phrase "Briev code in the crate" is literally true** —
167 lines of integer predicates — and it is also the only reason a consumer needs
`brievc` **at build time**.

---

## 2. The finding that matters more than the question

The seam between Counsil and Briev currently **treats silence as proof.**

The chain, with evidence:

| # | Fact | Where |
|---|---|---|
| 1 | Briev knows the proven names: `pub proven: Vec<String>` | `verify_bodies.rs:57` |
| 2 | but serialises only the **count**: `"proven": self.proven.len()` | `verify_bodies.rs:78` |
| 3 | Counsil asks for that JSON, then **discards it**: `proven: Vec::new()` | `check/brievc.rs:249` |
| 4 | and infers Proven as *"every defn not named unproven or violated"* | `check/brievc.rs:169,213` |
| 5 | Briev **silently skips** bodies it cannot model: `continue` | `verify_bodies.rs:130` |
| 6 | and silently skips when inlining fails: `continue` | `verify_bodies.rs:140` |

Therefore: **any declaration Briev never examined is reported `Proven` by Counsil.**
Not "unproven", not "skipped" — proven.

This has not yet produced a false proof, because Counsil's emitted vocabulary
(`defn`/`when`/assign/`term`/arithmetic) is entirely inside Briev's modelled set.
But it is a latent hole of exactly the kind this project must never ship, and it
exists **because** the interface is a subprocess plus prose rather than a typed
result. It is not caused by Briev's prover; it is caused by the *protocol*.

Crucially, the data needed to close it is already on the wire and already
computed: the JSON carries `proven` as a count.

### 2.1 Reachability: tested, and no live trigger found

A structural hole is only urgent if it is reachable, so I looked for a trigger
that Counsil actually emits. The strongest candidate was **parentheses**:
`translate_expr` reproduces source parens verbatim (`Expr::Paren` → `({inner})`),
and `expr_supported` has no paren arm, so any parenthesised subexpression would
have silently skipped the whole body.

**That hypothesis is refuted.** Briev's `Expr` has **no `Paren` variant**
(`ast/expr.rs`), and the parser never constructs one — parentheses are normalised
away, so `(a + b)` and `a + b` produce the same AST. Parenthesised source is not a
trigger.

No other trigger was found in Counsil's emitted vocabulary (`defn`, `when`,
assignment, `term`, integer literals, identifiers, `+ - * / %`, comparisons,
`&&`/`||`) — every one of those maps to a `stmt_supported`/`expr_supported` arm.

The honest statement is therefore: **the hole is structurally real but its
reachability is unproven.** That is still worth fixing, for three reasons:

1. It is ~10 lines and needs no Briev change.
2. It is testable with **pure data** — construct a `Discharge` and a defn list and
   assert the accounting; no `brievc` required.
3. It converts an unproven risk into a detection. Counsil's entire promise is that
   it never reports a proof it does not have; "we could not find a trigger" is not
   the same as "there is no trigger", and the guard costs nothing at runtime.

### 2.2 A correction to earlier documentation

A comment in `src/check/translate.rs` (and a note in the P4 design) says hairline
antecedents are emitted without parentheses because *"`encodable` rejects
`Expr::Paren`, which would silently skip the solver"*. **There is no
`Expr::Paren`**, so that reason is wrong. The P4 fix that mattered was carrying the
function precondition into the antecedent; the paren removal was incidental. The
stale comment should be corrected so the next reader does not chase a variant that
cannot exist.

---

## 3. Answering "pre-compile a binary to embed"

Treating this literally — vendor `brievc` and `include_bytes!` it — the facts are:

- **Size.** The current asset is **48,735,203 bytes compressed** (v0.1.3). The
  previous one (v0.1.2) was 9,883,386 bytes. crates.io caps a crate at **10 MB**.
  Embedding v0.1.3 is impossible; embedding v0.1.2 leaves no room for code.
- **Platforms.** We ship exactly one target today (`brievc-linux-x86_64`).
  Embedding makes a published crate single-platform unless we build a matrix
  (macOS x86_64/arm64, Windows, …). The download-by-URL design scales where
  embedding does not.
- **It does not remove Briev.** It ships the same Briev, with different packaging.

So embedding is not the lever. It changes *when* Briev is compiled, not *whether*
it is used.

---

## 4. Is Briev earning its place?

Split the answer by half.

### Static prover — yes, substantially

- ~1,900 lines of soundness-critical engine (symbolic path walk, bounds, SMT
  encoding) that we did not write and would have to own and prove correct.
- It has already acted as an **independent implementation checking ours**: this
  session it caught two real translator bugs that no Counsil test caught —
  `undefined variable 'best'` (a guarded assignment we failed to parameterise)
  and the parenthesised antecedent that `encodable` rejected *silently*.
  A single implementation would have shipped both.
- Briev also owns the Z3 encode/decode quirks (`bvsdiv`/`bvsrem`, bitvector
  wraparound) — semantics we would otherwise get subtly wrong.

### Runtime half — weakly

- 167 lines, and the interesting part (callbacks) had to be abandoned for an ABI
  bug. What remains is integer predicates over a flat Int ABI, composed by the
  Rust facade.
- Its real cost is the **build-time `brievc` requirement for every consumer** —
  the single largest adoption tax in the crate.
- Its real value is dogfooding: the project's thesis is "the predicate core is
  authored in Briev". Removing it would make that thesis true only of the static
  half.

### Costs we have actually paid

| Cost | Evidence |
|---|---|
| Version-pin fragility | 0.1.6 shipped loop invariants against a compiler predating all three fixes they need; consumer saw `Unproven` where I saw `Proven` |
| Text round-trip fragility | three emitter/encoding bugs this session (parens rejected by `encodable`, antecedent shape, self-reference double-expansion) |
| Distribution weight | 48 MB asset vs a ~1 MB crate |
| Silence-as-proof | §2 above |

---

## 5. Options

| Option | Change | Removes | Costs |
|---|---|---|---|
| **A. Status quo** | — | — | 48 MB asset, version pinning, silence-as-proof |
| **B. Vendor/embed prebuilt binary** | delivery | build-time fetch | 10 MB crate limit blocks it; per-target matrix; still Briev |
| **C. Link `briev_compiler` as a library** | interface | subprocess, stderr prose, version download, *and* the text round-trip (could build AST directly) | 32 deps + compiling a full compiler per consumer build; heavy for a verifier |
| **D. Reimplement the prover in Counsil** | ownership | Briev entirely for static | owns ~1,900 lines of soundness-critical code; **loses the independent cross-check** that caught two bugs |
| **E. Keep Briev, fix the protocol** | verdict handling | silence-as-proof | ~10 lines in Counsil; optionally one line in Briev |

Note on C: `briev-lang` *is* a library (`[lib] name = "briev_compiler"`,
`src/lib.rs`, `pub mod analysis`) and has no LLVM/inkwell dependency — so C is
technically open. It is the cleanest way to remove the *protocol* risk, because
verdicts become a typed `BodyReport` instead of prose. It is not worth doing
today: compiling a compiler per consumer build costs far more than it saves.

---

## 6. Recommendation

**Do E now. Decide the runtime half separately. Do not embed a binary.**

### E1 — fail closed on the count (do this first, ~10 lines, no Briev change)

Counsil already receives `{"proven": <count>, ...}`. It discards the count. Instead:

```
expected = defns.len() - unproven.len() - violated.len()
```

- If `proven_count == expected` → the accounting is complete; inference is safe.
- If `proven_count < expected` → **some declarations were skipped**. Report them
  `Unproven` with a reason ("the prover did not report on this declaration"), never
  `Proven`.

This converts silence from *proof* into a detected inconsistency, using data
already on the wire. It is the single highest-value change in this document.

### E2 — make Briev report honestly (small, upstream, optional but right)

Two one-line options, both cheap because the data already exists:
- emit `"proven": [{"name": ...}]` instead of `.len()`, or
- add `"skipped": [names]` to `BodyReport`.

Somewhere to put it: `verify_bodies` currently `continue`s without a trace; a
`skipped` vector would let Counsil say *why* a declaration has no verdict.

### E3 — split the runtime half from the prover

The build-time `brievc` requirement exists solely to compile 167 lines of integer
predicates. Two honest routes:

- **Keep it (status quo):** preserve the dogfooding thesis, accept build-time
  `brievc` and an ABI bug class.
- **Port it to Rust:** ~30 lines of predicates, removes build-time `brievc`
  entirely, so a default-features consumer needs **no Briev at all**; Briev then
  appears only under `--features static`, where it is already a downloaded binary.
  Cost: the "Briev-authored core" claim narrows to the static half.

This is a product/identity decision, not a technical one — it needs your call.
The technical fact is that the runtime half is the *only* place dropping Briev is
cheap, and it is where the adoption cost concentrates.

---

## 7. Concrete plan

1. **E1** — add the accounting check in `check/brievc.rs`; record the count in
   `Discharge`; fail closed. Tests: (a) a report whose counts balance still yields
   `Proven`; (b) a report with fewer proven than expected yields `Unproven` and
   names the declarations; (c) a compiler that writes no report still falls back to
   stderr without regressing.
2. **Prove the hole is real** in the lab before fixing it: craft a defn that Briev
   skips (e.g. one containing an unmodelled statement) and confirm Counsil currently
   reports `Proven`, then confirm it reports `Unproven` after E1. A fix without a
   failing test first is a guess.
3. **E2** — propose the `skipped` field to `briev-lang` (we own it), so the reason
   reaches the user instead of a generic "not reported".
4. **E3** — decide runtime-half ownership; if porting to Rust, that is its own plan.
5. Release only after (2) demonstrates the before/after on the consumer path.

## 8. Direct answer: can Counsel work without a brievc dependency?

**Yes — and it decomposes into two independent questions that should not be
conflated.**

### 8.1 The build-time use is cheap to remove, and is the real tax

`build.rs` runs `brievc` on `bridge/counsel.bv` (**167 lines**) purely to obtain a
native library of **integer predicates** (`add`, `within`, `step_up`,
`run_checked`, `verify_samples`). That is roughly **30 lines of Rust** to replace.

Removing it means a consumer with default features needs **no Briev at all** — no
download, no compiler, no ABI bug class. Briev then appears only under
`--features static`.

The only thing lost is dogfooding for that half (the "Briev-authored core" claim
narrows to the static half). That is a product decision, not a technical one.

### 8.2 The check-time use is a prover, and its closure is small

Measured directly from the source:

| File | Lines | Real logic | Imports |
|---|---|---|---|
| `analysis/verify_bodies.rs` | 1,543 | **~1,225** (tests start at 1,226) | `ast`, `symbolic` |
| `symbolic.rs` | 737 | 737 | `ast` only |
| `proof_engine/smt.rs` | 345 | 345 | `ast`, `Command` |
| **Total** | **2,625** | **~2,307** | **`crate::ast` types only** |

The four `crate::library` references in `verify_bodies.rs` are **all inside
`mod tests`** (lines 1234–1508). Nothing in the prover's logic reaches into the
parser, typechecker, backends or codegen. The 32-dependency, 48 MB `brievc`
monolith is the **compiler** — and for static proof Counsel uses none of it.

Routes, with what each costs:

| Route | Size of new/changed code | Removes | Keeps | Loses |
|---|---|---|---|---|
| **A. Carve a `briev-verify` library crate** (ast subset + symbolic + verify_bodies + smt) | ~2,300 lines *moved*, near-zero written | brievc binary, 48 MB fetch, version pin, subprocess, stderr prose, text round-trip, **and the silence-as-proof hole** (typed `BodyReport`) | the engine, and a separate AST (partial cross-check) | nothing material |
| **B. Port the prover into Counsel on the syn AST** | ~900–1,200 lines *written* | Briev entirely | a single language, direct AST→SMT | **the independent implementation** — it caught 2 real translator bugs this session; and a Briev typechecker that caught a third |
| **C. Link `briev_compiler` wholesale** | 0 | subprocess + version pin | everything | consumer build time (compiles a compiler; 32 deps) |
| **D. Embed a prebuilt binary** | 0 | fetch | everything | **infeasible**: 48.7 MB vs the 10 MB crates.io crate limit, plus a per-target build matrix |

### 8.3 The dependency you do *not* remove: z3

`smt.rs` invokes `z3` as a subprocess (`z3_bin()` → `Command::new`). Today the
chain is:

```
Counsel ──> brievc ──> z3
```

Every route above yields:

```
Counsel ──> z3
```

So "no brievc dependency" is achievable; "no external process" is a **different**
project. Options for z3, none free:

- link `libz3` via the `z3` crate → still a native library at build/link time;
- keep the `z3` binary (status quo);
- drop z3 entirely → **regresses proofs we already ship**. Interval reasoning
  cannot do relational facts, and relational facts are exactly what loop
  invariants rest on (`acc == i ⟹ acc + 1 == i + 1`). `count_preserve` would go
  from `proven` back to `Unproven`.

z3 is load-bearing. Removing `brievc` is a real win; removing `z3` is a
different, much larger decision.

### 8.4 Recommendation

1. **Do 8.1 first** (port the 167-line runtime half to Rust). It is small, it
   removes the largest adoption tax, and it needs no prover decision.
2. **For the prover, take Route A** if the goal is "no brievc": move the ~2,300
   logic lines into a `briev-verify` crate, and have Counsel depend on it. This
   eliminates the binary, the fetch, the version pin *and* the silence-as-proof
   protocol hole in one move, while keeping the engine that has been catching our
   bugs.
3. **Do not take Route B** unless the goal is explicitly to depend on no Briev
   code at all. It is more work than A and it gives up the independent
   implementation — the single most valuable property Briev has contributed to
   this project so far.

### 8.5 Before committing to any route, verify

**Measured (task 37) — the carve-out is bounded and cheap.**

`crate::ast` has exactly **one** external dependency in the whole module tree:
`crate::errors::Span`. There is no path from the AST to the typechecker, the
stdlib loader, the parser or any backend. Concretely:

| Piece | Lines | Needed by the prover? |
|---|---|---|
| `ast/expr.rs` | 388 | **yes** — `Expr`, `Pattern`, `DerivationBlock` |
| `ast/top.rs` | 1,674 | **yes** — `Statement`, `Contract`, `TopLevel` |
| `ast/types.rs` | 315 | **yes** — `Type`, `BitRange`, `Formatting`, `PropertyValue` |
| `ast/canonical.rs` | 953 | no |
| `ast/display.rs` | 871 | no |
| `ast/visit.rs` / `visit_mut.rs` | 464 / 441 | no |
| `ast/bad.rs` / `bld.rs` | 402 / 198 | no |
| `errors.rs` | 1,710 | only `Span`, a 4-field struct (~20 lines to shim) |

Those three AST files reference **only each other** (`crate::ast::Type`,
`super::Statement`, `super::Formatting`, …); none pulls `canonical`, `display`,
`visit`, `bld` or `bad`. So:

```
Route A moves:  2,307  (verify_bodies 1,225 + symbolic 737 + smt 345)
              + 2,377  (expr.rs + top.rs + types.rs)
              +   ~20  (Span shim, instead of 1,710 lines of errors.rs)
              ≈ 4,700 lines MOVED, ~0 written
```

The AST half is pure data — structs and enums with no behaviour — so moving it is
mechanical. That removes the only unbounded cost I could see in Route A: it is
bounded, and it is cheap.

Remaining checks before committing:

- **Compile the carved crate.** The measurement is file-level `use` inspection; the
  compiler is the authority. Expect a handful of unused-variant warnings, which is
  fine for a `-p briev-verify` crate.
- **That the silence-as-proof hole is real** (see §2), with a failing test first.
  It is worth fixing whether or not the architecture moves.

## 10. Would this carve-out be "a Briev compatibility crate"?

It depends entirely on **one** design choice, and the two answers are 5× apart in
size. "Compatibility crate" can mean three different things here; only one of them
is a trap.

### 10.1 As an adapter/compat *layer* — no

There is no interface to translate. Counsil would call the prover with typed
values in, a typed `BodyReport` out. Nothing is being adapted; a contiguous slice
of Briev is being **extracted**. That is an ordinary library relationship (think
`rustc_ast`, not a shim).

### 10.2 As a *fork* — only if we copy instead of extract upstream

If `briev-verify` is populated by copying `ast/` + the prover into a new crate, we
own two copies of the AST and the two drift silently. That is a compatibility
burden wearing a library's clothes, and it reintroduces version skew in a subtler
form than the one we are removing.

The fix is that **`briev-lang` itself depends on `briev-verify`** — the prover and
the AST types it needs live in the leaf crate, and the compiler consumes them.
Then there is exactly one copy and the relationship is a normal semver constraint.

Note: `briev-lang/Cargo.toml` has **no `[workspace]`** today. Upstream extraction
means creating one — a real structural change to Briev, worth doing deliberately.

### 10.3 As "Counsil keeps speaking Briev" — **this is the trap**

This is the reading that matters, and it is decided by whether Counsil keeps
emitting `.bv` **source text**.

| | Carries | Size | Typecheck safety net |
|---|---|---|---|
| **A1 — keep text** | parser **17,725 lines** + lexer + `vocab` + `type_universe` | **≈ 22,500** | kept |
| **A2 — build AST directly** | `expr`+`top`+`types` (2,377) + prover (2,307) | **≈ 4,700** | **lost** |

A1 is literally a vendored Briev front end, and the parser is not cleanly
separable: it references `crate::typechecker` (`definitions.rs:8321`) and
`crate::analysis::desugar` (`expressions.rs:642`) in production code, not tests.
Shrinking the carve-out *only* by keeping the text round-trip would drag the
compiler in through the back door — the exact thing we are trying to remove.

A2 works today without asking Briev to change: `MatchArm` and `Contract` expose
`pub` fields, `Contract::new(pre, post)` exists, and **enum variant fields are
implicitly public** for a public enum, so every node Counsil needs is constructible
from outside the crate.

**A2's cost is the typechecker.** When Counsil emits text, `brievc` parses and
**type-checks** it, and that is what caught `undefined variable 'best'` — a real
bug in our emitter that no Counsil test found. Building the AST directly removes
that free check. If we go A2, the replacement is Counsil-side validation of its
own emitted vocabulary (cheap, because the vocabulary is small and closed), but it
is our check against our code rather than an independent one.

### 10.4 So: not a compatibility crate — if we choose "prover library"

The clean target is **A2, extracted upstream**: `briev-verify` holds the symbolic
prover plus the AST types it reasons over; `briev-lang` depends on it too; Counsil
builds AST nodes directly and gets a typed verdict. No parser, no text, no binary,
no version pin — and no fork.

The thing to avoid is drifting into **A1** by keeping text out of convenience. That
is the version that would genuinely be "a Briev compatibility crate", and it is
five times the code.

## 11. Consumer lock-in: what "no Briev dependency" actually means

**Today the answer is no**, and the reason is not what the phrase suggests.

### 11.1 There is no Briev *crate* in the graph — the lock is a build-time binary

`counsel`'s dependencies are `counsel-macros`, `syn` and `serde_json` — **no Briev
crate at all**. `cargo tree` looks perfectly clean. The lock-in is invisible to it:

```
build.rs::main()
  if DOCS_RS { return; }          // the ONLY short-circuit
  let brievc = resolve_brievc();  // unconditional, feature-independent
  build_machinery(&brievc, …);    // compiles bridge/counsel.bv
```

Features gate *code* (`macros`, `static`), never the build script. So even
`cargo add counsel --no-default-features` downloads a 48 MB compiler, sha256-checks
it and compiles a `.bv` file. That is a harder dependency than a crate: invisible
in `cargo tree`, platform-pinned to linux x86_64, version-pinned by hash, and
unbuildable offline.

| Layer | Today | After porting the 167-line bridge |
|---|---|---|
| build-time toolchain (`brievc`) | **required, always** | **gone** |
| check-time toolchain (`brievc`) | required for `--features static` | still required there (until A2 or Route B) |
| crate graph | no Briev crate | `briev-verify` only under `static`, or none |
| runtime `z3` | required for `static` | required for `static` |

### 11.2 The default path is ~60 lines away from zero Briev

The whole native surface is `src/bindings.rs` — **60 lines**, generated — wrapping
about seven integer predicates (`add`, `within`, `is_positive`, `is_nonneg`, `eq`,
`run_checked`, `verify_samples`) implemented in `bridge/counsel.bv` (167 lines,
8 KB). Porting those to Rust is ~30 lines and deletes:

- `build.rs`'s entire resolve/download/sha256/build machinery,
- `bridge/counsel.bv`, `src/bindings.rs`, the `extern "C"` linkage,
- the 48 MB asset, the version pin, and the `COUNSEL_BRIEVC_PATH` baking.

After that, `cargo add counsel` yields a dependency graph of `counsel` →
`counsel-macros` (a pure-Rust proc-macro crate), builds offline, on any platform,
and **never invokes Briev**. The `#[pre]`/`#[post]`/`#[invariant]` attributes are
already pure Rust; only the build script was forcing the toolchain.

**So the user-facing goal is achieved by the 30-line port (task 38), not by the
carve-out.** The carve-out only matters for the optional `static` feature.

### 11.3 For `static` there are exactly three options — no fourth

Static proof requires a prover. It must arrive as one of:

1. **Briev's prover, linked as a crate** (`briev-verify`, A2) — ordinary Rust
   compiled into the consumer's build. No toolchain, no binary, no download. The
   crate appears in `cargo tree`, which is honest: it *is* Briev code.
2. **Our own reimplementation** (Route B) — no Briev code, but we own ~900–1,200
   lines of soundness-critical logic and lose the independent implementation.
3. **A binary** (status quo) — the thing we are trying to remove.

There is no version of "static proof with no prover from anywhere". The honest goal
is therefore: *Briev as an implementation detail of an optional feature, never as a
build-time or runtime toolchain the user must obtain.*

### 11.4 Precise statement of the answer

> **Can we use Briev to build Counsel without locking users into a Briev dependency?**

- **Default features:** yes — after task 38 (~30 lines). Today, no: every build
  fetch-compiles a 48 MB compiler.
- **`--features static`:** no toolchain lock-in after A2 — but Briev's prover is
  then linked as *code*, which is a categorically weaker form of dependence than a
  binary and is the unavoidable minimum unless Route B is taken.
- **`z3`** remains required for `static` either way.

## 12. What this means for "does Briev earn its place"

- As a **prover**: yes — it is ~2,300 lines of soundness-critical engine plus a
  second implementation that has caught bugs in ours.
- As a **predicate host at runtime**: not really — 167 lines whose interesting
  design was abandoned for an ABI bug, and it forces `brievc` onto every consumer's
  build.
- As an **interface**: it is currently the weakest link, and that is a defect on our
  side of the seam, not Briev's.
