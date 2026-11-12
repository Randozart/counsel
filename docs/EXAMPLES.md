# Counsel — Examples

Worked end-to-end walks through both halves. Every compiler output below is
copied verbatim from a real run (`brievc` built from `briev-lang` `c0ce60dc`),
not paraphrased.

---

## 1. Static proof: a contract that holds

Rust source (what you write):

```rust
#[pre(a >= 0)]
#[post(term == a + a)]
fn double(a: i64) -> i64 { a + a }
```

Generated Briev (what `counsel-static` emits for the prover):

```briev
defn double(a: Int) [a >= 0][term == a + a] -> Int {
    term a + a;
};
```

Result:

```
$ brievc build double.bv --library --verify-bodies --out out/
exit 0            # no line mentions `double`
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
$ brievc build lies.bv --library --verify-bodies --out out/
error: --verify-bodies:
lies: body can violate its postcondition: a path returns term = 5, which contradicts the contract
exit 1
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
$ brievc build scale.bv --library --verify-bodies --out out/
warning: --verify-bodies could not prove: scale: could not prove the postcondition on 1 path(s) — add a guard or a stronger precondition
exit 0
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
    "unsupported expression (pure-integer subset): … — counsel-static verifies a \
     pure-integer subset: `i64` parameters and return, integer arithmetic, \
     comparisons, `let`, `if`/`else`, `return`, and a trailing expression"
))
```

Rejecting is deliberate: approximating a call would mean proving something
about a function the prover cannot see.

---

## 6. From a test

```rust
use counsel_static::{check_source, Outcome};

let report = check_source(r#"
    #[pre(a >= 0)]
    #[post(term == a + a)]
    fn double(a: i64) -> i64 { a + a }
"#)?;

assert_eq!(report.verdicts[0].outcome, Outcome::Proven);
assert!(report.all_proven());
# Ok::<(), counsel_static::Error>(())
```

Functions without `#[pre]`/`#[post]` are ignored, so a file can mix
contracted and uncontracted code freely.

**Integration note.** `counsel-static` reads source *text* and parses it with
`syn`; it does not compile your crate, and `#[pre]`/`#[post]` are
attributes this tool understands (there is no proc-macro yet). Call
`check_source`/`check_file` from a test, a build step, or CI.

---

## 7. Runtime checks (`counsel`)

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

## 8. What each outcome means for your build

| Outcome | Build impact | Meaning | What to do |
|---|---|---|---|
| `Proven` | passes | every path establishes the postcondition | nothing |
| `Unproven` | passes, warns | decidable body, undecided obligation | strengthen the pre/guard, simplify the post |
| `Violated` | **fails** | a concrete counterexample exists | fix the body or the contract |
| `Error::Unsupported` | fails | construct outside the pure-integer subset | rewrite, or verify by hand |
| `Error::NoCompiler` | fails | no `brievc` with `--verify-bodies` found | set `BRIEVC` |

---

## Reproduce

```bash
# The three static verdicts, directly against the compiler:
cd /tmp && mkdir -p ex && cd ex
BRIEVC=/path/to/brievc   # a build of briev-lang c0ce60dc or later

printf 'defn double(a: Int) [a >= 0][term == a + a] -> Int {\n    term a + a;\n};\n' \
  > proven.bv
"$BRIEVC" build proven.bv --library --verify-bodies --out out-proven     # exit 0

printf 'defn lies(a: Int) [a >= 0][term == 0] -> Int {\n    term 5;\n};\n' \
  > violated.bv
"$BRIEVC" build violated.bv --library --verify-bodies --out out-violated # exit 1

printf 'defn scale(a: Int) [true][term == a * a] -> Int {\n    term a * 2;\n};\n' \
  > unproven.bv
"$BRIEVC" build unproven.bv --library --verify-bodies --out out-unproven # exit 0, warning
```

```bash
# Through the crate's own API:
cd counsel-crate
BRIEVC=/path/to/brievc cargo test -p counsel-static   # translator + real prover
BRIEVC=/path/to/brievc cargo test -p counsel          # builds + links Briev machinery
```

---

## 9. Intended cases (`#[case]`)

`#[post]` says what must hold for *every* input. `#[case]` pins inputs you
actually care about, so the prover can give a definite answer instead of
`Unproven`:

```rust
#[pre(a >= 0)]
#[post(term == 2 * a)]
#[case(5)]
fn double(a: i64) -> i64 { a + a }
```

The general goal is undecidable by structure here (`a + a` versus `2 * a`), so
the translator also emits the contract instantiated at the case's input:

```briev
defn double__case0() [5 >= 0][term == 2 * 5] -> Int { term 5 + 5; };
```

which the prover collapses to constants and decides.

| Verdict | Meaning |
|---|---|
| `Proven` | symbolic: every path satisfies `#[post]` |
| `ProvenForCase` | satisfied **at that input only** — reported as `double (case 0)` |
| `Violated` | a concrete counterexample exists |
| `Unproven` | decidable body, undecided obligation |

`Report::all_proven()` counts only symbolic `Proven`, so a case can never make a
report green.

Two deliberate refusals: a case whose arity does not match the parameters is a
contract error, and a function that rebinds a parameter (`let a = 1;`) rejects
cases outright — the substitution is textual, and guessing would be worse than
refusing.

Syntax notes: `#[case(5)]` for one parameter, `#[case((2, 3))]` for several.
There is no `=> expected` form: an expectation that cannot be checked would be a
silent weakness. The expectation comes from `#[post]`, which *is* proved at that
input.
