// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// tests/smoke.rs — Phase 0 end-to-end smoke test.
//
// Proves the full path: build.rs compiles bridge/counsel.bv to a static
// library, links it, and these tests drive the Briev-authored exports through
// the safe facade. If this passes, the scaffold spike's exit criteria are met.

use counsel::{add, contract, eq, in_range, nonneg, positive, verify, within, Verdict};

/// The Briev-authored `add` export computes a sum through the C ABI.
#[test]
fn add_is_briev_computed() {
    assert_eq!(add(3, 4), 7);
    assert_eq!(add(-2, -5), -7);
    assert_eq!(add(0, 0), 0);
}

// --- Phase 1 (A1, reliable form): Briev-computed checks + Rust composition ---

/// All obligations hold for a positive input.
#[test]
fn contract_passes_when_held() {
    let body = |x: i64| x * 2;
    assert_eq!(
        contract(counsel::positive, body, |_x, ret| counsel::nonneg(ret), 5),
        Verdict::Pass
    );
    assert_eq!(
        contract(counsel::positive, body, |_x, ret| counsel::nonneg(ret), 1),
        Verdict::Pass
    );
}

/// A negative input violates the precondition; body and post never run.
#[test]
fn contract_fails_precondition() {
    let body = |x: i64| x * 2;
    assert_eq!(
        contract(counsel::positive, body, |_x, ret| counsel::nonneg(ret), -3),
        Verdict::Fail("precondition failed".into())
    );
}

/// A body whose output violates the postcondition is reported as post-failure.
#[test]
fn contract_fails_postcondition() {
    // pre accepts positive input; body negates it; post demands positive output.
    assert_eq!(
        contract(
            counsel::positive,
            |x: i64| -x,
            |_x, ret| counsel::positive(ret),
            7
        ),
        Verdict::Fail("postcondition failed".into())
    );
}

/// Briev-computed predicate wrappers agree with their Verdict-facing forms.
#[test]
fn briev_predicates_agree_with_verdicts() {
    assert!(in_range(5, 0, 10));
    assert!(!in_range(11, 0, 10));
    assert!(positive(3));
    assert!(!positive(-2));
    assert!(nonneg(0));
    assert!(!nonneg(-1));
}

/// `within` passes for an in-range value.
#[test]
fn within_passes_in_range() {
    assert_eq!(within(5, 0, 10), Verdict::Pass);
    assert_eq!(within(0, 0, 10), Verdict::Pass); // inclusive lower bound
    assert_eq!(within(10, 0, 10), Verdict::Pass); // inclusive upper bound
}

/// `within` fails for out-of-range values with a naming reason.
#[test]
fn within_fails_out_of_range() {
    match within(11, 0, 10) {
        Verdict::Fail(reason) => {
            assert!(
                reason.contains("11"),
                "reason should name the value: {reason}"
            )
        }
        Verdict::Pass => panic!("11 should not satisfy [0, 10]"),
    }
    assert_eq!(
        within(-1, 0, 10),
        Verdict::Fail(format!("-1 not in [0, 10]"))
    );
}

// --- Phase 2: cross-verification sampler ---

/// Briev-computed equality agrees for equal/unequal pairs.
#[test]
fn eq_is_briev_computed() {
    assert!(eq(3, 3));
    assert!(eq(-7, -7));
    assert!(!eq(1, 2));
    assert!(!eq(0, 1));
}

/// Original and shadow that agree over every sample pass.
#[test]
fn verify_passes_when_shadow_agrees() {
    assert_eq!(
        verify(|x| x * 2, |x| x * 2, 20, |i| i as i64),
        Verdict::Pass
    );
}

/// A boundary case: agreement at zero samples trivially passes.
#[test]
fn verify_passes_with_zero_samples() {
    assert_eq!(verify(|x| x, |x| x + 999, 0, |_| 0), Verdict::Pass);
}

/// The first divergence is caught and named with input/expected/actual.
#[test]
fn verify_catches_first_divergence() {
    // shadow adds 1; original adds 0 -> diverges at sample 0 (input 0).
    assert_eq!(
        verify(|x| x, |x| x + 1, 5, |i| i as i64),
        Verdict::Fail("sample 0 diverged on input 0: expected 1, got 0".into())
    );
}

/// Divergence on a later sample is still caught at the first mismatch.
#[test]
fn verify_catches_divergence_on_later_sample() {
    // both double up to input 9, but shadow switches to +1 at input >= 10.
    let shadow = |x: i64| if x < 10 { x * 2 } else { x * 2 + 1 };
    assert_eq!(
        verify(|x| x * 2, shadow, 20, |i| i as i64),
        Verdict::Fail("sample 10 diverged on input 10: expected 21, got 20".into())
    );
}
