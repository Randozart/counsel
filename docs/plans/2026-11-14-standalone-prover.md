# Making Counsel standalone: a purpose-built prover

**Status:** in progress (2026-11-14).

## Decision

Counsel stops depending on `briev-verify`. It gets its own prover, written for
exactly the nodes Counsel emits, and the `briev-verify` dependency is removed.

Briev remains Randy's language and keeps its own prover — this is not a
divergence from it, it is Counsel no longer needing it.

## Why

`briev-verify` is ~8,100 lines because it carries **Briev's general AST**: `bad`,
`bld`, visitors, canonicalisers, display for a whole language. Counsel uses five
expression kinds and four statement kinds.

Worse, the prover carries machinery Counsel never exercises:

| In `briev-verify` | Counsel's need |
|---|---|
| value summaries + call inlining | none — Counsel inlines on the `syn` tree first |
| loop support | none — loops become separate obligations (`_init`/`_preserve`/`_exit`) |
| `Match`, `Foreach`, `Yield`, sync blocks, metadata | none — refused before the prover sees them |
| Briev type model | none — every value is an `i64` |

So Counsel is paying for a general engine to run a straight-line walk.

## What the prover has to do

Only this:

- walk a straight-line body, forking on `when`/`if` guards;
- track assignments and facts, tighten integer intervals;
- decide a path: structurally, by interval, or by SMT;
- aggregate per-declaration verdicts: `Proven` / `Unproven` / `Violated`.

No loops, no calls, no types, no summaries. That is a few hundred lines, not
8,100.

## Shape

```
src/check/prover/
  mod.rs        public entry: verify(&[Defn]) -> Report
  ast.rs        the five expressions, four statements, contract
  symbolic.rs   substitution, constant folding, interval bounds
  smt.rs        SMT-LIB encoding + z3 invocation (unchanged semantics)
  walk.rs       path walk + tri-state verdict + aggregation
```

## Invariants that must survive the port

These were learned the hard way and are not negotiable:

1. **A path's facts are recorded in the ENTRY state.** Substituting the final
   assignment state into them rewrote an entry fact into the goal and produced a
   **false proof** (briev-lang `ed2fc83d`).
2. **A self-referential assignment must not double-expand**, and the cycle guard
   must be applied *before* descending (`acc = acc + 1` is `acc+1`, not `acc+2`;
   briev-lang `95dbe54c`).
3. **`Sat` with a free variable is advisory, never a refutation.** Only a fully
   determined model refutes.
4. **Silence is not proof.** A declaration the prover never examined is
   `Unproven`, never `Proven`.
5. **`int` wraps at 64 bits**, so interval reasoning must not assume otherwise.

## Gate

The existing suites are the specification — no test may be weakened:

- 21 lib, 22 smoke, 11 valve, 9 doc-tests in `counsel`;
- `scripts/acceptance.sh` (6 lab-derived cases) must pass against the published
  artefact.

Then publish `counsel 0.3.0` — the first release with no Briev dependency — and
run `./scripts/acceptance.sh 0.3.0`.

## What changes for users

Nothing, except one line in `cargo tree`. The pitch becomes simpler: a
contract checker with no exotic dependencies, whose prover is its own.

## What is given up

The mutual-improvement loop. Two real prover bugs surfaced this session *because*
Counsel exercised `briev-verify` and Briev fixed them. After this, a defect in
Counsel's prover is Counsel's alone, and vice versa. That is the cost of
standalone, and it should be stated plainly rather than glossed.

`briev-verify` stays published and stays Briev's prover. Nothing is yanked.
