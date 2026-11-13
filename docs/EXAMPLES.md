# Counsel — Examples

Worked end-to-end walks through both halves. Every verdict and reason below comes
from a real run of the in-process prover, not paraphrased.

---

## 1. Static proof: a contract that holds

Rust source (what you write):

```rust
#[pre(a >= 0)]
#[post(term == a + a)]
fn double(a: i64) -> i64 { a + a }
```

Equivalent Briev (the prover's own syntax; Counsel builds this AST directly):

```briev
defn double(a: Int) [a >= 0][term == a + a] -> Int {
    term a + a;
};
```

Result:

```
prover report: 1 proven, 0 unproven, 0 violated
```

Outcome:

```rust
Outcome::Proven
```

The prover walked the single path, bound the result of `a + a`, and matched it
structurally against the postcondition — no numeric value needed.

---

## 2. Static proof: a contract that is false

The classic case that used to slip through — a body contradicting its own
postcondition.

```rust
#[pre(a >= 0)]
#[post(term == 0)]
fn lies(a: i64) -> i64 { 5 }
```

Generated Briev:

```briev
defn lies(a: Int) [a >= 0][term == 0] -> Int {
    term 5;
};
```

Result (verbatim):

```
prover report: 1 violated
lies: body can violate its postcondition: a path returns term = 5, which contradicts the contract
```

Outcome:

```rust
Outcome::Violated     // contiguous counterexample, not a guess
```

`5` versus the required `0` is decided on concrete values, so this is a hard
failure — and the message names the offending value.

---

## 3. Static proof: the prover cannot decide

Honesty matters more than a high "proved" count. Here the postcondition is
arithmetically false for most inputs, but the prover has no numeric value to
compare and no interval facts that would separate `a * 2` from `a * a`.

```rust
#[post(term == a * a)]
fn scale(a: i64) -> i64 { a * 2 }
```

Generated Briev:

```briev
defn scale(a: Int) [true][term == a * a] -> Int {
    term a * 2;
};
```

Result (verbatim):

```
prover report: 1 unproven
scale: could not prove the postcondition on 1 path(s) — add a guard or a stronger precondition
```

Outcome:

```rust
Outcome::Unproven     // advisory: not a pass, not an error
```

Two things to notice:

- The build still **succeeds** (exit 0). `Unproven` never blocks; it warns.
- Every run also prints advisories about *stdlib* declarations, e.g.
  `warning: --verify-bodies could not prove: coll_min: …`. Those are not your
  code: classification only ever asks about the translated names, so they are
  ignored.

**How to make this case decidable:** strengthen the contract so a fact exists
to reason with, or rewrite the body so the shapes match. There is no Z3 arbiter
yet — reasoning is interval bounds plus structural equality.

---

## 4. Static proof: branches on both arms

```rust
#[pre(a >= 0)]
#[post(term >= 0)]
fn clamp(a: i64) -> i64 { if a > 5 { return a; } 0 }
```

Generated Briev:

```briev
defn clamp(a: Int) [a >= 0][term >= 0] -> Int {
    when a > 5 { term a; };
    term 0;
};
```

Result: `exit 0`, no line mentions `clamp` ⇒ `Outcome::Proven`.

Why both paths hold: on the taken guard the facts include `a > 5` (so `a ≥ 6`,
hence `term ≥ 0`); on the fall-through the value is the literal `0`.

An `if`/`else` whose arms both produce a value compiles the same way — the else
body is emitted unguarded, because reaching it implies the guard failed:

```rust
#[post(term >= 0)]
fn pick(a: i64) -> i64 { if a > 5 { a } else { 0 } }
```
```briev
defn pick(a: Int) [true][term >= 0] -> Int {
    when a > 5 { term a; };
    term 0;
};
```

(When an arm can fall through, the else arm keeps an inverted guard such as
`when a <= 5 { … };` instead.)

---

## 5. Static proof: a construct outside the subset

```rust
#[post(term >= 0)]
fn f(a: i64) -> i64 { helper(a) }
```

Result — no compiler run, rejected during translation:

```rust
Err(Error::Unsupported(
    "unsupported expression (pure-integer subset): … — Counsel verifies a \
     pure-integer subset: integer parameters and return (i8-i64, u8-u32, u64/usize/isize with their range facts), integer arithmetic, \
     comparisons, `let`, `if`/`else`, `return`, and a trailing expression"
))
```

Rejecting is deliberate: approximating a call would mean proving something
about a function the prover cannot see.

---

## 6. Static proof: a loop, proved in three obligations

A `for` loop carrying `#[invariant(...)]` is **not** unrolled. It becomes three
loop-free obligations, all discharged by the same solver:

```rust
#[pre(n >= 0 && n <= 1000)]
#[post(term >= 0)]
#[invariant(acc == i && i <= n)]
pub fn count(n: i64) -> i64 {
    let mut acc = 0;
    for i in 0..n {
        acc = acc + 1;
    }
    acc
}
```

```
count_init        proven
count_preserve    proven
count_exit        proven
```

`_init` shows the invariant holds on entry, `_preserve` that the body keeps it,
`_exit` that it is enough to meet `#[post]`. **All three are required** — a
partial set proves nothing about the function, and a wrong invariant is caught at
the obligation it actually breaks:

```
count_bad_init    VIOLATED     // acc == i + 1 is false at entry
count_bad_preserve proven      // valid, but only conditional on the premise
count_bad_exit     proven
```

`Int` is a **wrapping** 64-bit bitvector, so an invariant must bound the values it
reasons about (a bare `acc >= 0` is not preserved — it can wrap) and must survive
the growth step. `n <= 1000` in `#[pre]` is what makes `acc == i && i <= n`
provable.

Integer methods `a.max(b)`, `a.min(b)` and `a.abs()` are supported; they desugar
into guarded branches, so a loop may end in `best.max(1)`. Any other method is
refused by name rather than approximated.

## 7. From a test

```rust
use counsel::check::{check_source, Outcome};

let report = check_source(r#"
    #[pre(a >= 0)]
    #[post(term == a + a)]
    fn double(a: i64) -> i64 { a + a }
"#)?;

assert_eq!(report.verdicts[0].outcome, Outcome::Proven);
assert!(report.all_proven());
# Ok::<(), counsel::check::Error>(())
```

Functions without `#[pre]`/`#[post]` are ignored, so a file can mix
contracted and uncontracted code freely.

**Integration note.** Counsel reads source *text* and parses it with `syn`; it
does not compile your crate. `#[pre]`/`#[post]`/`#[case]` are re-exported from
`counsel` as inert attributes, so the same file compiles in your crate. Call
`check_source`/`check_file` from a test, a build step, or CI.

---

## 8. Runtime checks (`counsel`)

The other half answers "did the obligation hold for *this* input?" — with the
truth values computed by Briev-compiled native code.

```rust
use counsel::{contract, nonneg, positive, Op, satisfies, verify, Verdict};

// pre → body → post, short-circuiting on the first failure.
assert_eq!(
    contract(positive, |x| x * 2, |_, ret| nonneg(ret), 5),
    Verdict::Pass
);
assert_eq!(
    contract(positive, |x| x * 2, |_, ret| nonneg(ret), -3),
    Verdict::Fail("precondition failed".into())
);

// Predicate-as-data: the opcode and operands cross into Briev, which decides.
assert!(satisfies(Op::In { lo: 0, hi: 10 }, 5));
assert!(!satisfies(Op::Lt(0), 5));

// Sampler: run a function beside a reference; compare outputs through Briev.
assert_eq!(verify(|x| x * 2, |x| x * 2, 20, |i| i as i64), Verdict::Pass);
assert_eq!(
    verify(|x| x * 2, |x| x * 2 + 1, 5, |i| i as i64),
    Verdict::Fail("sample 0 diverged on input 0: expected 1, got 0".into())
);
```

Pass `positive` / `nonneg` / `in_range` (or the `Op` form) to have Briev decide.
A hand-written closure such as `|x| x > 0` is evaluated by Rust, not Briev —
correct, but not the part this crate is exercising.

---

## 9. What each outcome means for your build

| Outcome | Build impact | Meaning | What to do |
|---|---|---|---|
| `Proven` | passes | every path establishes the postcondition | nothing |
| `Unproven` | passes, warns | decidable body, undecided obligation | strengthen the pre/guard, simplify the post |
| `Violated` | **fails** | a concrete counterexample exists | fix the body or the contract |
| `Error::Unsupported` | fails | construct outside the pure-integer subset | rewrite, or verify by hand |

---

## Reproduce

```bash
cargo test --features static                       # the whole static suite
cargo run --features static --bin counsel-check -- src/lib.rs
```

No compiler is involved: the prover is the `briev-verify` crate and runs in the
same process.
