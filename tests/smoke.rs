// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// tests/smoke.rs — Phase 0 end-to-end smoke test.
//
// Proves the full path: build.rs compiles bridge/counsel.bv to a static
// library, links it, and these tests drive the Briev-authored exports through
// the safe facade. If this passes, the scaffold spike's exit criteria are met.

use counsel::{add, within, Verdict};

/// The Briev-authored `add` export computes a sum through the C ABI.
#[test]
fn add_is_briev_computed() {
    assert_eq!(add(3, 4), 7);
    assert_eq!(add(-2, -5), -7);
    assert_eq!(add(0, 0), 0);
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
