# Roadmap — Counsel as a standalone tool

**Date:** 2026-11-12
**Status:** items 1–3 done. C1 (`2b361ed2`, `3c41fe6`), C6+C5 (`860abe10`),
T1 (`4574ca8`), T2+T3 (`088e631`). Next: C2 packaging; C3/C4 prover depth open.
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

## Item 4 — C2: packaging

Publish `briev-compiler` (or ship prebuilt `brievc` binaries) so `cargo add
counsel` works. Deliberately after the interface settles: the published shape
should not change twice.

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
