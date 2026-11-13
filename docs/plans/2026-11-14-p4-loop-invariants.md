# P4 requirements — dynamic loop bounds via invariants

**Date:** 2026-11-14
**Status:** Design only. Not implemented.
**Why:** measured 178 of 180 loop-bearing eligible functions use **dynamic**
bounds (`0..n`, `0..iters`). Constant unrolling (`b7d6910`) covers ~1 in 535.
Unrolling a variable trip count is unsound, so the only way forward is an
invariant.

## The key insight: the Hoare rule needs no induction

The obvious objection is that Briev's prover refuses recursion (C3/R4), so it
cannot do induction. **It does not have to.** The standard `while` rule turns one
loop into three *straight-line* verification conditions:

| VC | Obligation |
|---|---|
| 1 | `pre ⟹ I[lo, init]` |
| 2 | `I ∧ (i < hi) ⟹ I[ body ; i := i+1 ]` — preservation |
| 3 | `I ∧ ¬(i < hi) ⟹ post` |

Every one is loop-free, so Counsel's existing pipeline (translate a straight-line
body → `brievc --verify-bodies` → Z3) can discharge them **unchanged**. No
induction principle, no prover work.

This also explains why the current refusal is correct rather than lazy: without
`I`, VC2 has nothing to preserve and the check is vacuous.

## Annotation surface

```rust
#[post(term == n * (n - 1) / 2)]
#[invariant(0 <= acc)]          // names loop var and mutable locals
fn sum(n: i64) -> i64 {
    let mut acc = 0;
    for i in 0..n { acc = acc + i; }
    acc
}
```

Constraints to keep it honest:
- the invariant may mention the loop variable, the mutable locals, and the
  function parameters — nothing else;
- every local the loop writes must be mentioned, or the invariant is
  under-specified and VC2 will (correctly) fail;
- a loop with no `#[invariant]` keeps today's refusal, with the message already
  pointing here (`94a51db`).

## Translation plan

For a `for i in lo..hi { body }` with invariant `I`:

1. **Model state**: loop var `i` and each mutated local become symbolic names.
2. **VC1**: `defn vc1(<params>) [pre][I[i:=lo, locals:=init]] { term 1; }` — or
   express as a single postcondition over the initial state.
3. **VC2**: take the body's *straight-line* translation (assignment support
   already exists, `42853a7`), append `i = i + 1`, and require `I` to still hold.
4. **VC3**: assert `I ∧ i >= hi ⟹ post`.
5. Emit the three as ordinary parameterless/parameterised `defn`s and reuse
   `brievc --verify-bodies` per defn — the `check_source` path already handles
   multiple defns, and masked failures are now reported individually
   (`81a7240`).

## Soundness rules

- An **unannotated** dynamic loop stays refused. Never infer an invariant.
- A **vacuous** invariant (`true`) must be refused the way tautological contracts
  are — otherwise VC2 passes while proving nothing.
- Report the loop as "proved with the stated invariant": the guarantee is only as
  strong as what the user asserted, and the message should say so.

## Order of work

1. Parse `#[invariant(...)]` (mirror `contract_attr`, including the
   duplicate-slot rejection it already documents).
2. VC2 only — preservation is the load-bearing obligation and the one that fails
   on a wrong invariant. Ship it, then add VC1/VC3.
3. Re-run the lab: `optimal_step_for_cycle_length` (dynamic bounds) is the
   existing reproduction.
4. Update the ledger's tally and drop class (b) "dynamic loop bounds".

## Outcome — VC2 works (2026-11-14)

`count_preserve` proves end to end: a `for i in 0..n` loop with
`#[invariant(acc == i && i <= n)]` and `#[pre(n >= 0 && n <= 1000)]` discharges.

The route there corrected three of my own mistakes, all worth keeping:

1. **The antecedent must carry the function precondition.** A bound like
   `n <= 1000` is what makes the arithmetic wraparound-free; without it nothing
   bounded can prove.
2. **Never emit parentheses.** `encodable` rejects `Expr::Paren`, so a
   parenthesised conjunct does not fail loudly — it silently skips the solver.
3. **A self-referential assignment must not double-expand.** `acc = acc + 1`
   became `acc + 2` in the goal until the cycle guard was applied *before*
   descending into the value (briev-lang `95dbe54c`).

**No SSA renaming is needed.** I had concluded it was, and was wrong: entry-state
facts and a structurally substituted goal speak the same symbols, so the VC is
consistent as built. Reading the emitted query (`BRIEV_DIAG_VC`) showed the goal
was `acc+2`, which settled it in one step.

### Rule for users, now that this is real

`Int` is a **wrapping** 64-bit bitvector. An invariant must therefore
(a) bound the values it reasons about, and (b) survive the growth step. A bare
`acc >= 0` is unprovable (it can wrap) and `acc <= 1000` is not preserved by
`acc = acc + i` (`i` is unbounded). `acc == i && i <= n` with `n` bounded satisfies
both, which is why it proves.

## VC1 and VC3 — the loop is now fully proved (0e03f4d)

Preservation alone proved nothing about the *function*: it said the invariant
survives one step, not that it holds on entry or that it is worth anything at
exit. All three while-rule obligations are now emitted and discharged:

| defn | antecedent | goal |
|---|---|---|
| `<fn>_init` | `pre && locals == their initialisers && i == lo` | `I` |
| `<fn>_preserve` | `pre && I && i >= lo && i in range` | `I` |
| `<fn>_exit` | `pre && I && i past the end` | `post` |

`<fn>_exit` binds `term` to the trailing expression after the loop, which is how
`post` (stated in terms of `term`) is evaluated at exit. `lo..=hi` is treated as
one extra step.

### The decomposition is discriminating, which is the whole point

For a deliberately wrong invariant, `#[invariant(acc == i + 1 && i <= n)]`:

```
count_bad_init                   VIOLATED
count_bad_preserve               proven
count_bad_exit                   proven
```

The entry obligation fails, because `acc == i + 1` is false at entry. The other
two prove — correctly, since they are only claimed *conditional* on the premise.
This is why VC2 alone was misleading: it would have reported success. **A loop is
proved only when all three hold**, and a wrong invariant is caught at the
obligation whose premise it actually breaks.

### Limits, stated plainly

- The loop must be `lo..hi` over a `for` with a fixed variable, followed by a
  returned value.
- A mutated local with no initialiser before the loop contributes no pin, so
  `<fn>_init` is left unproven rather than assumed.
- Bounds must be constant for the *translator* to unroll; invariants are the
  escape from that, which is precisely why they matter.
