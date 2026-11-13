// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// src/lib.rs — Counsel crate facade.
//
// Safe Rust API for runtime contract checking. This half is **pure Rust**: it
// used to be a thin wrapper over native code compiled from `bridge/counsel.bv`
// by `build.rs`, which forced every consumer to obtain a Briev compiler before
// its build could start. The predicates are a handful of integer comparisons,
// so they are implemented here directly and the build machinery is gone.
//
// Briev is now an implementation detail of the *static* half only
// (`--features static`), which resolves a `brievc` binary at check time.
// Nothing in the default feature set touches Briev.

/// Contract attributes: `#[pre(...)]`, `#[post(...)]`, `#[case(...)]`.
///
/// These are **inert markers**: they exist so rustc accepts the syntax, and
/// contribute no runtime behaviour. Enforcement comes from [`check`] (static,
/// `--features static`) or from [`contract`] (runtime).
///
/// ```
/// use counsel::{post, pre};
///
/// #[pre(a >= 0)]
/// #[post(term == a + a)]
/// fn double(a: i64) -> i64 { a + a }
///
/// assert_eq!(double(3), 6);
/// ```
#[cfg(feature = "macros")]
pub use counsel_macros::{case, invariant, post, pre};

/// Static verification: translate a restricted pure-integer Rust subset and
/// discharge its contracts through Briev's own body prover.
///
/// Enabled with `--features static`; the `counsel-check` binary ships with it.
#[cfg(feature = "static")]
pub mod check;

/// The type produced by a Counsel check predicate: pass or fail with a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The obligation held.
    Pass,
    /// The obligation failed; `reason` names the violated condition.
    Fail(String),
}

/// Wrapping integer addition.
///
/// Wraps on overflow, preserving the semantics of the native implementation this
/// replaces: its `Int` was a wrapping 64-bit word, so ordinary `+` would panic in
/// a debug build exactly where the old code silently wrapped.
pub fn add(a: i64, b: i64) -> i64 {
    a.wrapping_add(b)
}

/// Whether `x` lies in the inclusive range `[lo, hi]`.
///
/// # Example
/// ```
/// use counsel::in_range;
/// assert!(in_range(5, 0, 10));
/// assert!(!in_range(11, 0, 10));
/// ```
pub fn in_range(x: i64, lo: i64, hi: i64) -> bool {
    lo <= x && x <= hi
}

/// Whether `x` is strictly positive.
pub fn positive(x: i64) -> bool {
    x > 0
}

/// Whether `x` is non-negative.
pub fn nonneg(x: i64) -> bool {
    x >= 0
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

/// Whether two integers are equal.
pub fn eq(a: i64, b: i64) -> bool {
    a == b
}

/// Runs a pre → body → post check with the ordering enforced here.
///
/// `pre` guards entry, `body` runs only when `pre` holds, and `post` sees the
/// input and the body's result. The return code mirrors the previous native
/// implementation exactly: `0` pass, `1` failed precondition, `2` failed
/// postcondition.
pub fn run_checked(
    pre: extern "C" fn(i64) -> i64,
    body: extern "C" fn(i64) -> i64,
    post: extern "C" fn(i64, i64) -> i64,
    x: i64,
) -> Verdict {
    if pre(x) != 1 {
        return Verdict::Fail("precondition failed".into());
    }
    let ret = body(x);
    if post(x, ret) != 1 {
        return Verdict::Fail("postcondition failed".into());
    }
    Verdict::Pass
}

/// Cross-verifies `original` against a `shadow` reference over `n` samples.
///
/// Sample inputs are the sample index itself (`0..n-1`), so no generator has to
/// be passed in. Each pair is compared and the first divergence is reported.
///
/// Returns [`Verdict::Pass`] when every sample agrees, otherwise
/// [`Verdict::Fail`] naming the first divergent sample.
pub fn verify_samples(
    original: extern "C" fn(i64) -> i64,
    shadow: extern "C" fn(i64) -> i64,
    n: usize,
) -> Verdict {
    // A sample count beyond i64 range is clamped rather than truncated.
    let samples = i64::try_from(n).unwrap_or(i64::MAX);
    for sample in 0..samples {
        let expected = shadow(sample);
        let actual = original(sample);
        if actual != expected {
            return Verdict::Fail(format!(
                "sample {sample} diverged: original({sample}) != shadow({sample})"
            ));
        }
    }
    Verdict::Pass
}

/// A `<,>,<=,>=,in-range` predicate held as a tagged value.
///
/// Kept as data (rather than a closure) so a predicate can be stored,
/// compared and reported.[`satisfies`] evaluates it.
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

/// Whether `op` holds for `x`.
///
/// # Example
/// ```
/// use counsel::{Op, satisfies};
/// assert!(satisfies(Op::Gt(0), 5));
/// assert!(satisfies(Op::In { lo: 0, hi: 10 }, 5));
/// assert!(!satisfies(Op::Lt(0), 5));
/// ```
pub fn satisfies(op: Op, x: i64) -> bool {
    match op {
        Op::Gt(a) => x > a,
        Op::Ge(a) => x >= a,
        Op::Lt(a) => x < a,
        Op::Le(a) => x <= a,
        Op::In { lo, hi } => lo <= x && x <= hi,
    }
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
    note = "prefer `verify_samples`: it takes no generator closure and clamps the \
            sample count safely. This form keeps a Rust loop for generator-driven \
            inputs."
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
