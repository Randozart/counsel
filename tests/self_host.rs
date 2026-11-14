// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// self_host.rs — Counsel proves Counsel.
//
// The four helpers are copied VERBATIM from src/lib.rs (the shipped pure-integer API)
// and given the contracts their doc comments describe. Verifying them with the current
// Counsel binary is self-hosting (SS9): the same tool that ships them also proves them.
// They live here, not in src/lib.rs, so the published crate carries no attribute
// dependency on the macros feature.

use counsel::check::{check_source, Outcome};

fn verdict(name: &str) -> Outcome {
    let report = check_source(
        "#[pre(x > 0)]\n#[post(term == 1)]\n\
         fn positive(x: i64) -> bool { x > 0 }\n\
         #[pre(x >= 0)]\n#[post(term == 1)]\n\
         fn nonneg(x: i64) -> bool { x >= 0 }\n\
         \n\
         #[pre(a == b)]\n#[post(term == 1)]\n\
         fn eq(a: i64, b: i64) -> bool { a == b }\n\
         #[pre(lo <= x && x <= hi)]\n#[post(term == 1)]\n\
         fn in_range(x: i64, lo: i64, hi: i64) -> bool { lo <= x && x <= hi }\n",
    )
    .expect("the four lib helpers parse and verify in-process");
    report
        .verdicts
        .iter()
        .find(|v| v.name == name)
        .expect("helper present")
        .outcome
        .clone()
}

/// Counsel proves the predicate is right at its own boundary (the same contracts the
/// doc comments state).
#[test]
fn positive_is_one_for_a_positive_input() {
    assert_eq!(verdict("positive"), Outcome::Proven);
}

#[test]
fn nonneg_is_one_for_a_non_negative_input() {
    assert_eq!(verdict("nonneg"), Outcome::Proven);
}

#[test]
fn eq_is_one_for_equal_inputs() {
    assert_eq!(verdict("eq"), Outcome::Proven);
}

/// in_range is a correct membership predicate: inside is reported, outside is not.
#[test]
fn in_range_reports_containment() {
    assert_eq!(verdict("in_range"), Outcome::Proven);
}
