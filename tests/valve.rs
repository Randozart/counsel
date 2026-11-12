// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// tests/valve.rs — end-to-end Phase 4 valve tests.
//
// Each test translates Rust source into Briev `defn`s and discharges them
// through the real `brievc --verify-bodies` gate. Tests that need the compiler
// return early when none is installed, so the suite stays green on machines
// without a Briev checkout (the unit tests still cover the translator).

use counsel::check::{check_source, Outcome, Report};

/// Whether a Briev compiler with `--verify-bodies` is available.
fn have_compiler() -> bool {
    counsel::check::brievc::find_compiler().is_some()
}

/// Verify a snippet, skipping the test when no compiler is installed.
fn verify(source: &str) -> Option<Report> {
    if !have_compiler() {
        return None;
    }
    Some(check_source(source).expect("verification should not error"))
}

/// A body that provably satisfies its postcondition is discharged statically.
#[test]
fn proven_contract_is_discharged() {
    let Some(report) =
        verify("#[pre(a >= 0)]\n#[post(term == a + a)]\nfn double(a: i64) -> i64 { a + a }\n")
    else {
        return;
    };
    assert_eq!(report.verdicts.len(), 1);
    assert_eq!(report.verdicts[0].outcome, Outcome::Proven);
    assert!(report.all_proven());
}

/// A constant body that contradicts its postcondition is a hard violation.
#[test]
fn false_contract_is_violated() {
    let Some(report) = verify("#[pre(a >= 0)]\n#[post(term == 0)]\nfn lies(a: i64) -> i64 { 5 }\n")
    else {
        return;
    };
    assert_eq!(report.verdicts[0].outcome, Outcome::Violated);
    assert_eq!(report.violated().len(), 1);
    assert!(!report.all_proven());
}

/// An undecidable contract is advisory (Unproven), never a violation.
#[test]
fn undecidable_contract_is_advisory() {
    // `a * a` vs `a * 2` is neither structurally equal nor decidable by
    // interval bounds: the prover must say "could not prove", not "violated".
    let Some(report) = verify("#[post(term == a * a)]\nfn scale(a: i64) -> i64 { a * 2 }\n") else {
        return;
    };
    assert_eq!(report.verdicts[0].outcome, Outcome::Unproven);
    assert!(report.violated().is_empty());
    assert_eq!(report.unproven().len(), 1);
}

/// A path-split body (early return + fall-through) is proven on every path.
#[test]
fn path_split_body_is_proven() {
    let Some(report) = verify(
        "#[pre(a >= 0)]\n#[post(term >= 0)]\n\
         fn clamp(a: i64) -> i64 { if a > 5 { return a; } 0 }\n",
    ) else {
        return;
    };
    assert_eq!(report.verdicts[0].outcome, Outcome::Proven);
}

/// `else` branches are proven through the inverted guard.
#[test]
fn else_branch_is_proven() {
    let Some(report) = verify(
        "#[pre(a >= 0)]\n#[post(term >= 0)]\n\
         fn pick(a: i64) -> i64 { if a > 5 { a } else { 0 } }\n",
    ) else {
        return;
    };
    assert_eq!(report.verdicts[0].outcome, Outcome::Proven);
}

/// Several declarations in one file are verified independently.
#[test]
fn multiple_declarations_are_verified_independently() {
    let Some(report) = verify(
        "#[pre(a >= 0)]\n#[post(term == a + 1)]\nfn inc(a: i64) -> i64 { a + 1 }\n\
         #[pre(a >= 0)]\n#[post(term == 0)]\nfn bad(a: i64) -> i64 { 3 }\n",
    ) else {
        return;
    };
    assert_eq!(report.verdicts.len(), 2);
    assert_eq!(report.verdicts[0].outcome, Outcome::Proven);
    assert_eq!(report.verdicts[1].outcome, Outcome::Violated);
}

/// Uncontracted functions are ignored entirely.
#[test]
fn uncontracted_functions_are_ignored() {
    let Some(report) = verify("fn plain(a: i64) -> i64 { a }\n") else {
        return;
    };
    assert!(report.verdicts.is_empty());
}

/// Unsupported constructs are rejected with a clear, non-compiler error.
#[test]
fn unsupported_constructs_are_rejected() {
    let err = check_source("#[post(term >= 0)]\nfn f(a: i64) -> i64 { helper(a) }\n")
        .expect_err("calls are outside the subset");
    assert!(err.to_string().contains("pure-integer subset"), "{err}");
}

/// A `#[case]` is proved end-to-end at its input, is reported under a readable
/// name, and never counts as a general proof.
#[test]
fn case_is_proven_at_its_input() {
    // The general goal is undecidable by structure (`a + a` vs `2 * a`), so the
    // case is the only thing that can decide it — at a = 5.
    let Some(report) = verify(
        "#[pre(a >= 0)]\n#[post(term == 2 * a)]\n#[case(5)]\n\
         fn double(a: i64) -> i64 { a + a }\n",
    ) else {
        return;
    };
    let case = report
        .verdicts
        .iter()
        .find(|v| v.outcome == Outcome::ProvenForCase)
        .unwrap_or_else(|| panic!("no ProvenForCase verdict: {:?}", report.verdicts));
    assert_eq!(case.name, "double (case 0)");
    assert!(
        !report.all_proven(),
        "a case must never make a report green: {:?}",
        report.verdicts
    );
}
