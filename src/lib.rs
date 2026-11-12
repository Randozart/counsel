// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// src/lib.rs — Counsel crate facade.
//
// Safe Rust API over the Briev-authored verification machinery. The heavy
// lifting lives in bridge/counsel.bv (compiled to native by build.rs); this
// module wraps the C-ABI bindings so callers get checked, documented entry
// points instead of raw `extern` calls.
//
// Phase 1 note (Ambition A1, reliable form): the checks crossed the C ABI as
// flat `extern "C" fn(i64)` bookeps presented a runtime ABI mismatch in Briev
// (see BUGS.md). The reliable design keeps the *predicate bodies* authored in
// Briev as pure Int-ABI exports (`within`, `is_positive`, `is_nonneg`) and
// composes them from Rust via closures. Orchestration order + value plumbing
// live in the Rust facade; each obligation's truth value is Briev-computed.

mod bindings;

/// The type produced by a Counsel check predicate: pass or fail with a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The obligation held.
    Pass,
    /// The obligation failed; `reason` names the violated condition.
    Fail(String),
}

/// Briev-verified integer addition.
///
/// Returns `a + b` as computed by the Briev-authored `add` export.
///
/// # Safety
/// The `add` export is pure C ABI with no state dependency; this wrapper is
/// safe to call from any thread.
pub fn add(a: i64, b: i64) -> i64 {
    // Safety: `add` is a pure C ABI export with no state dependency.
    unsafe { bindings::add(a, b) }
}

/// Whether `x` lies in the inclusive range `[lo, hi]`, computed by Briev.
///
/// # Example
/// ```
/// use counsel::in_range;
/// assert!(in_range(5, 0, 10));
/// assert!(!in_range(11, 0, 10));
/// ```
pub fn in_range(x: i64, lo: i64, hi: i64) -> bool {
    // Safety: `within` is a pure C ABI export with no state dependency.
    unsafe { bindings::within(x, lo, hi) == 1 }
}

/// Whether `x` is strictly positive, computed by Briev.
pub fn positive(x: i64) -> bool {
    // Safety: `is_positive` is a pure C ABI export with no state dependency.
    unsafe { bindings::is_positive(x) == 1 }
}

/// Whether `x` is non-negative, computed by Briev.
pub fn nonneg(x: i64) -> bool {
    // Safety: `is_nonneg` is a pure C ABI export with no state dependency.
    unsafe { bindings::is_nonneg(x) == 1 }
}

/// Checks whether `x` is inside the inclusive range `[lo, hi]`.
///
/// Returns [`Verdict::Pass`] when `lo <= x <= hi`, otherwise [`Verdict::Fail`].
pub fn within(x: i64, lo: i64, hi: i64) -> Verdict {
    if in_range(x, lo, hi) {
        Verdict::Pass
    } else {
        Verdict::Fail(format!("{x} not in [{lo}, {hi}]"))
    }
}

/// Checks whether `x` is strictly positive.
pub fn is_positive(x: i64) -> Verdict {
    if positive(x) {
        Verdict::Pass
    } else {
        Verdict::Fail(format!("{x} is not > 0"))
    }
}

/// Checks whether `x` is non-negative.
pub fn is_nonneg(x: i64) -> Verdict {
    if nonneg(x) {
        Verdict::Pass
    } else {
        Verdict::Fail(format!("{x} is negative"))
    }
}

/// Runs a pre/body/post contract over one integer argument.
///
/// `pre: x -> bool` guards entry, `body: x -> ret` is the function under
/// check, and `post: (x, ret) -> bool` verifies the outcome. Orchestration
/// order is enforced here (pre runs first; body only if pre holds; post only
/// if body ran). The predicate closures may delegate their truth values to the
/// Briev-authored checks ([`positive`], [`nonneg`], [`in_range`]) — that is the
/// Ambition-A1 division of labour: Rust composes, Briev computes.
///
/// Returns [`Verdict::Pass`] when both obligations hold; otherwise
/// [`Verdict::Fail`] naming which obligation (pre vs post) was violated.
///
/// # Example
/// ```
/// use counsel::{Verdict, contract};
/// let body = |x: i64| x * 2;
/// // pre: positive input; post: output non-negative.
/// let ok = contract(counsel::positive, body, |_x, ret| counsel::nonneg(ret), 5);
/// assert_eq!(ok, Verdict::Pass);
/// let bad = contract(counsel::positive, body, |_x, ret| counsel::nonneg(ret), -3);
/// assert_eq!(bad, Verdict::Fail("precondition failed".into()));
/// ```
pub fn contract(
    pre: impl Fn(i64) -> bool,
    body: impl Fn(i64) -> i64,
    post: impl Fn(i64, i64) -> bool,
    x: i64,
) -> Verdict {
    if !pre(x) {
        return Verdict::Fail("precondition failed".into());
    }
    let ret = body(x);
    if !post(x, ret) {
        return Verdict::Fail("postcondition failed".into());
    }
    Verdict::Pass
}

/// Whether two integers are equal, computed by Briev (`eq`).
///
/// The sampler [`verify`] uses this to compare each original-vs-shadow output
/// pair through Briev-computed equality.
pub fn eq(a: i64, b: i64) -> bool {
    // Safety: `eq` is a pure C ABI export with no state dependency.
    unsafe { bindings::eq(a, b) == 1 }
}

/// Runs a Briev-orchestrated pre → body → post check.
///
/// The ordering and short-circuiting live in Briev (`bridge/counsel.bv`); this
/// wrapper only marshals the callbacks across the C ABI and maps the verdict
/// code. The callbacks are plain `extern "C"` functions — since briev-lang
/// 43519b01 the Briev export adapts a flat host pointer into its internal
/// closure slot, so a Rust closure with `extern "C"` linkage works directly.
///
/// Returns [`Verdict::Pass`] when both obligations hold, otherwise a
/// [`Verdict::Fail`] naming which obligation fired.
pub fn run_checked(
    pre: extern "C" fn(i64) -> i64,
    body: extern "C" fn(i64) -> i64,
    post: extern "C" fn(i64, i64) -> i64,
    x: i64,
) -> Verdict {
    // Safety: the export is pure C ABI; each callback is `extern "C"` and
    // therefore safe to invoke from foreign code.
    let code = unsafe { bindings::run_checked(pre, body, post, x) };
    match code {
        0 => Verdict::Pass,
        1 => Verdict::Fail("precondition failed".into()),
        2 => Verdict::Fail("postcondition failed".into()),
        other => Verdict::Fail(format!("unknown verdict code {other}")),
    }
}

/// Cross-verifies `original` against a `shadow` reference over `n` samples,
/// with the sampling loop authored in Briev (`bridge/counsel.bv`).
///
/// Sample inputs are the sample index itself (`0..n-1`), so no generator has
/// to cross the ABI. Briev walks the samples, compares each pair and reports
/// the first divergence; this wrapper only marshals the two callbacks and maps
/// the result.
///
/// Returns [`Verdict::Pass`] when every sample agrees, otherwise
/// [`Verdict::Fail`] naming the first divergent sample.
pub fn verify_samples(
    original: extern "C" fn(i64) -> i64,
    shadow: extern "C" fn(i64) -> i64,
    n: usize,
) -> Verdict {
    // Safety: the export is pure C ABI; both callbacks are `extern "C"`.
    // A sample count beyond i64 range is clamped rather than truncated.
    let samples = i64::try_from(n).unwrap_or(i64::MAX);
    let code = unsafe { bindings::verify_samples(original, shadow, samples) };
    if code < 0 {
        Verdict::Pass
    } else {
        Verdict::Fail(format!(
            "sample {code} diverged: original({code}) != shadow({code})"
        ))
    }
}

/// A `<,>,<=,>=,in-range` predicate transported to Briev as data (Ambition A2).
///
/// The predicate is *not* a Rust closure: it is a tagged variant, serialised to
/// an integer opcode plus its operand(s), and evaluated by the Briev-authored
/// `eval_pred` export. This is the pure-Briev-predicate path under the
/// cross-ABI-callback blocker (see BUGS.md) — data crosses the pure Int ABI;
/// Briev owns the truth value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `x > a`.
    Gt(i64),
    /// `x >= a`.
    Ge(i64),
    /// `x < a`.
    Lt(i64),
    /// `x <= a`.
    Le(i64),
    /// `a <= x <= b`.
    In { lo: i64, hi: i64 },
}

impl Op {
    /// The Briev `eval_pred` opcode for this variant.
    fn code(self) -> i64 {
        match self {
            Op::Gt(_) => 0,
            Op::Ge(_) => 1,
            Op::Lt(_) => 2,
            Op::Le(_) => 3,
            Op::In { .. } => 4,
        }
    }

    /// The operands `(a, b)` transported to Briev; `b` is unused for the
    /// unary variants.
    fn operands(self) -> (i64, i64) {
        match self {
            Op::Gt(a) => (a, 0),
            Op::Ge(a) => (a, 0),
            Op::Lt(a) => (a, 0),
            Op::Le(a) => (a, 0),
            Op::In { lo, hi } => (lo, hi),
        }
    }
}

/// Whether `op` holds for `x`, evaluated in Briev (`eval_pred`).
///
/// The predicate is passed to Briev as data; Briev computes the truth value.
///
/// # Example
/// ```
/// use counsel::{Op, satisfies};
/// assert!(satisfies(Op::Gt(0), 5));
/// assert!(satisfies(Op::In { lo: 0, hi: 10 }, 5));
/// assert!(!satisfies(Op::Lt(0), 5));
/// ```
pub fn satisfies(op: Op, x: i64) -> bool {
    let (a, b) = op.operands();
    // Safety: `eval_pred` is a pure C ABI export with no state dependency.
    unsafe { bindings::eval_pred(op.code(), a, b, x) == 1 }
}

/// Verdict form of [`satisfies`]: [`Verdict::Pass`] when `op` holds for `x`.
pub fn check_op(op: Op, x: i64) -> Verdict {
    if satisfies(op, x) {
        Verdict::Pass
    } else {
        Verdict::Fail(format!("{x} fails {:?}", op))
    }
}

/// Cross-verifies `original` against a `shadow` reference over `n` samples.
///
/// For each sample `i` in `0..n`, the input generator `gen(i)` is fed to both
/// `original` and `shadow`; the outputs must be Briev-equal on every sample.
/// This ports Briev's `cross_verify_samples` / `:=` idea: run the function
/// under test alongside a reference, assert IO-equivalence, report the first
/// divergence. Sampling, ordering, and the verdict are Rust-side; each
/// equality check is Briev-computed (`eq`).
///
/// Returns [`Verdict::Pass`] when all `n` samples agree, otherwise
/// [`Verdict::Fail`] naming the first divergent sample, its input, the
/// expected (shadow) output, and the actual (original) output.
///
/// # Example
/// ```
/// use counsel::{Verdict, verify};
/// // original and shadow both double their input; they agree on every sample.
/// let ok = verify(|x| x * 2, |x| x * 2, 20, |i| i as i64);
/// assert_eq!(ok, Verdict::Pass);
/// // a divergent shadow (adds 1) is caught at sample 0: input 0, expected 1, actual 0.
/// let bad = verify(|x| x * 2, |x| x * 2 + 1, 5, |i| i as i64);
/// assert_eq!(bad, Verdict::Fail("sample 0 diverged on input 0: expected 1, got 0".into()));
/// ```
#[deprecated(
    note = "prefer `verify_samples`: its sampling loop is authored in Briev. \
            This form keeps a Rust loop for generator-driven inputs."
)]
pub fn verify(
    original: impl Fn(i64) -> i64,
    shadow: impl Fn(i64) -> i64,
    n: usize,
    gen: impl Fn(usize) -> i64,
) -> Verdict {
    for i in 0..n {
        let input = gen(i);
        let expected = shadow(input);
        let actual = original(input);
        if !eq(actual, expected) {
            return Verdict::Fail(format!(
                "sample {i} diverged on input {input}: expected {expected}, got {actual}"
            ));
        }
    }
    Verdict::Pass
}
