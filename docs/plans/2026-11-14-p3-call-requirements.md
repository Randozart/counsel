# P3 requirements — calls and recursion in the Rust translator

**Date:** 2026-11-14
**Status:** Requirements investigation. No code changed.
**Repro:** `briev-counsel-lab/src/backend/llvm/loop_engine/analysis.rs::gcd`
(`if b == 0 { a } else { gcd(b, a % b) }`) — passes the width gate, stops at
`Expr::Call`.

## The finding that sizes the work

`translate_expr(expr: &syn::Expr, allow_ident: bool)` is **context-free**. There
is no place to ask "what does `gcd` return?", and no way to carry a summary map
without changing the signature of the whole expression layer:

| fn | line | needs context |
|---|---|---|
| `translate_expr` | 495 | yes |
| `translate_binary` / `translate_unary` | — | yes (they recurse) |
| `translate_local` | — | yes (let initialisers) |
| `translate_expr_stmt` | 354 | yes |
| `translate_stmt` | 326 | yes |
| `translate_block` | 316 | yes |
| `translate_fn` | 86 | yes (builds the summary) |

So P3 is **not** "add a `Call` arm". It is a context thread through ~7
signatures plus their call sites, then the feature on top. That is the honest
estimate, and it is why a rushed version would be a plausible-looking hole in a
verifier.

## Requirements

### R1 — a translation context
`struct Ctx { summaries: HashMap<String, Summary>, stack: Vec<String>, depth: usize }`
threaded by reference through the functions above. Mechanical, ~7 signatures.

### R2 — summary = a substitutable expression, not text
A callee's summary must be its returned **`syn::Expr`**, so a call site can
substitute arguments and then translate normally. Parsing the already-emitted
Briev text would be fragile and is rejected.

Derivable only when the callee has:
- only integer parameters (M1 already decides this),
- a **single-path** body — no early `return`, no `if` that returns on one branch,
- exactly one value: the trailing expression, or the sole `return`.

`gcd` fails this: its body is a multi-path `if`/`else *expression* — which is
fine actually, because an `if`-expression has one value; what must be excluded is
*statements* that return early. This distinction needs care in R2's predicate.

### R3 — soundness at the call site (the subtle one)
Substituting the callee's body assumes the callee's **precondition holds at the
call site**. Counsel does not check callee preconditions today.

Cheap and sound: **only summarise callees whose precondition is absent or
`true`**. A callee with a real precondition makes the call unsupported (and that
becomes a new ledger blocker class). Anything else would let a call be proved
under an assumption the caller never established — a false proof, the one
failure mode this project must never ship.

### R4 — recursion is refused, not guessed
A name already on the inlining `stack` yields no summary; the call is rejected
as today. Depth cap on substitution as a second guard. `gcd` therefore stays
blocked after P3 — recorded as its own blocker class so the frequency table can
say whether recursion is worth solving (it needs an induction principle or a
bounded unrolling argument, which is a different feature).

### R5 — tests
1. call to a summary-able callee → proved;
2. two-link chain → proved;
3. recursion → rejected (no false proof);
4. callee with a real precondition → rejected (R3);
5. callee with an early `return` → not summarised.

## Sequence

1. R1 context thread, no behaviour change (all existing tests must stay green —
   this is the risky mechanical step, and it is separable).
2. R2 summary construction + `Call` in `translate_expr`.
3. R3 precondition gate; R4 cycle refusal.
4. R5 tests; then re-run the lab: expect `gcd` still blocked, and add a
   non-recursive caller to the ledger as a positive case.

## Recommendation

Do R1 **alone** as one commit. If the mechanical thread touches more than
expected, we learn that before any behaviour rides on it — and the suite is the
gate. Then R2/R3/R4 together, since a partial version either proves nothing
(R2 without R3 is unsound) or blocks everything (R3 without R2).

## Outcome (implemented, `7a11a17`)

R1 was replaced by a smaller design that achieves the same thing: calls are
resolved on the `syn` tree **before** translation (`build_summaries` + an inliner
using `syn`'s `visit-mut`), so `translate_expr` stays a context-free pure printer
and **no signatures changed**. The seven-signature `Ctx` thread the estimate
assumed was unnecessary.

R2-R4 held unchanged: summary = the callee's returned `syn::Expr`; only callees
with no explicit precondition are summarised; a cycle earns no summary and its
residual call is rejected. `gcd` therefore still blocks (one level unrolled, then
refused — no false proof), and slice 3 proved a real caller end-to-end.
