// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// tests/valve.rs — end-to-end Phase 4 valve tests.
//
// Each test translates Rust source into Briev `defn`s and discharges them
// through the real `brievc --verify-bodies` gate. Tests that need the compiler
// return early when none is installed, so the suite stays green on machines
// without a Briev checkout (the unit tests still cover the translator).

use counsel::check::{check_source, Outcome, Report};

/// Verify a snippet. No compiler is involved: the prover runs in process, so
/// there is nothing to skip when it is absent.
fn verify(source: &str) -> Option<Report> {
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
    // An unmodelled *call* is now treated as opaque, with the assumption recorded in
    // Report.warnings. A construct with no value model at all is still refused rather
    // than guessed — that is the valve this test guards.
    let err = check_source("#[post(term >= 0)]\nfn f(a: i64) -> i64 { a[0] }\n")
        .expect_err("indexing is outside the subset");
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

/// The dominant real-world loop shape: a bound naming a local, and an update
/// guarded by an `if`. Both must reach the obligation's parameters, or the
/// emitted defn names an undefined variable and the obligation is lost.
#[test]
fn guarded_update_with_dynamic_bound_is_proven() {
    let Some(report) = verify(
        "#[post(term >= 1)]\n#[invariant(best >= 1)]\nfn optimal_step_plain(len: usize) -> usize { let mut best = 1; let half = len / 2; for d in 1..=half { if len % d == 0 { best = d; } } best }\n",
    ) else {
        return;
    };
    assert_eq!(
        report.verdicts.len(),
        3,
        "expected entry, preservation and exit: {:?}",
        report.verdicts
    );
    assert!(report.all_proven(), "not proven: {:?}", report.verdicts);
}

/// A construct outside the subset is refused by name. The message used to
/// interpolate the whole syntax tree, which is accurate and unreadable.
#[test]
fn unsupported_construct_is_named_not_dumped() {
    // Integer methods are supported and an unmodelled method is now opaque, so the
    // refusal must be tested with something genuinely outside the subset. `?` is the
    // right choice: it is refused deliberately, because treating it as opaque would
    // ignore its early-return path and could prove a postcondition the error branch
    // violates.
    let err = check_source("#[post(term >= 1)]\nfn f(a: Result<i64, i64>) -> i64 { a? }\n")
        .expect_err("`?` has no translation");
    let text = format!("{err}");
    assert!(
        text.contains("`?`"),
        "expected a construct name: {text}"
    );
    assert!(
        !text.contains("Expr::MethodCall"),
        "the message must not dump the syntax tree: {text}"
    );
}
