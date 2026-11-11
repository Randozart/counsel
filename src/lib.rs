// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// src/lib.rs — Counsel crate facade.
//
// Safe Rust API over the Briev-authored verification machinery. The heavy
// lifting lives in bridge/counsel.bv (compiled to native by build.rs); this
// module wraps the C-ABI bindings so callers get checked, documented entry
// points instead of raw `extern` calls.
//
// Phase 0 note: the current exports (`add`, `within`) are pure — they carry no
// state parameter — so no runtime state arena is required. Stateful machinery
// (register_contract/run_checked in Phase 1) will bring in the
// __briev_init_state handle at that point.

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

/// Checks whether `x` is inside the inclusive range `[lo, hi]`.
///
/// Returns [`Verdict::Pass`] when `lo <= x <= hi`, otherwise
/// [`Verdict::Fail`]. This is the seed of the obligation evaluator: the
/// predicate body is authored in Briev (`within`), the verdict the Rust side
/// reads from.
pub fn within(x: i64, lo: i64, hi: i64) -> Verdict {
    if within_bool(x, lo, hi) {
        Verdict::Pass
    } else {
        Verdict::Fail(format!("{x} not in [{lo}, {hi}]"))
    }
}

/// Raw boolean form of [`within`], backed directly by the Briev export.
///
/// # Safety
/// The `within` export is pure C ABI with no state dependency; this wrapper is
/// safe to call from any thread.
fn within_bool(x: i64, lo: i64, hi: i64) -> bool {
    // Safety: `within` is a pure C ABI export with no state dependency.
    unsafe { bindings::within(x, lo, hi) == 1 }
}
