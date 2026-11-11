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
