// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// Counsel — static verification (the Phase 4 valve).
//
// Static pre/post-condition verification for a restricted pure-integer Rust
// subset. The prover runs **in process**: `build` turns Rust syntax into the AST
// that `briev-verify` walks, and `verify_bodies` returns the verdicts. There is
// no compiler to find, download or version-pin, and no Briev source text.
//
//   #[pre(a >= 0)]
//   #[post(term >= 0)]
//   fn double(a: i64) -> i64 { a * 2 }
//
// Scope is deliberately narrow. Everything outside it is rejected rather than
// approximated: a wrong translation would be a wrong proof.

pub mod build;
pub mod inprocess;
pub mod prover;

use std::path::Path;

/// Why a verification attempt could not produce verdicts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The Rust source did not parse.
    Parse(String),
    /// The function used a construct outside the pure-integer subset.
    Unsupported(String),
    /// A contract declaration itself is malformed (for example a duplicated
    /// slot). Distinct from `Unsupported` so the message carries no subset hint
    /// that would mislead.
    Contract(String),
    /// Filesystem failure.
    Io(String),
}

impl std::fmt::Display for Error {
    /// Render a user-facing explanation with the concrete fix where one exists.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Parse(why) => write!(f, "cannot parse Rust source: {why}"),
            Error::Unsupported(why) => write!(
                f,
                "{why} — Counsel verifies a pure-integer subset: integer \
                 parameters and return (i8-i64, u8-u32, and u64/usize/isize with \
                 their range facts), integer arithmetic, comparisons, `let`, \
                 `if`/`else`, `return`, constant `for` ranges or a loop with an \
                 `#[invariant(...)]`, and a trailing expression"
            ),
            Error::Contract(why) => write!(f, "{why}"),
            Error::Io(why) => write!(f, "I/O error: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// Per-declaration verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The prover discharged the postcondition for every path.
    Proven,
    /// Decidable body, but the prover could not conclude (advisory).
    Unproven,
    /// A concrete counterexample exists — the contract is false.
    Violated,
    /// The contract holds at a `#[case]` input only. **Not** a general proof:
    /// `Report::all_proven` deliberately does not count it.
    ProvenForCase,
}

/// One verified declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnVerdict {
    /// The Rust function name.
    pub name: String,
    /// What the prover concluded.
    pub outcome: Outcome,
}

/// One function that was **not** checked, and why.
///
/// A skip is not a verdict: it records that nothing was proved. Without it a file
/// full of unannotated functions is indistinguishable from a file that passed, and
/// no census over a real codebase is possible — see
/// `docs/plans/2026-11-14-counsel-stress-test.md` (gap G0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    /// The Rust function name.
    pub name: String,
    /// Why it was not checked, in words.
    pub reason: String,
}

/// Verdicts for one source file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// One entry per contracted function, in source order.
    pub verdicts: Vec<FnVerdict>,
    /// One entry per function that was not checked, with a named reason.
    /// An empty `skips` plus empty `verdicts` means the file had no functions.
    pub skips: Vec<Skip>,
    /// Advisory messages: functions skipped because they fall outside the
    /// subset. A warning never means "verified" — it means nothing was proved.
    pub warnings: Vec<String>,
}

impl Report {
    /// Whether every contracted function was proven.
    pub fn all_proven(&self) -> bool {
        self.verdicts.iter().all(|v| v.outcome == Outcome::Proven)
    }

    /// The declarations with a falsified contract.
    pub fn violated(&self) -> Vec<&FnVerdict> {
        self.verdicts
            .iter()
            .filter(|v| v.outcome == Outcome::Violated)
            .collect()
    }

    /// The declarations the prover could not decide.
    pub fn unproven(&self) -> Vec<&FnVerdict> {
        self.verdicts
            .iter()
            .filter(|v| v.outcome == Outcome::Unproven)
            .collect()
    }
}

/// Verify the contracted functions in a Rust source string.
///
/// Functions without `#[pre]`/`#[post]`/`#[invariant]` are ignored — only
/// written contracts are obligations.
pub fn check_source(source: &str) -> Result<Report, Error> {
    inprocess::check_source(source)
}

/// Verify the contracted functions in a Rust file.
pub fn check_file(path: &Path) -> Result<Report, Error> {
    let source =
        std::fs::read_to_string(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    check_source(&source)
}

/// Readable name for a verdict: `<fn>__caseN` becomes `<fn> (case N)`.
///
/// The generated definition name keeps the case distinguishable while checking;
/// the report should speak about the function the author wrote.
pub(crate) fn display_name(name: &str) -> String {
    match name.split_once("__case") {
        Some((base, index)) => format!("{base} (case {index})"),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_helpers_split_outcomes() {
        let report = Report {
            warnings: Vec::new(),
            skips: Vec::new(),
            verdicts: vec![
                FnVerdict {
                    name: "a".into(),
                    outcome: Outcome::Proven,
                },
                FnVerdict {
                    name: "b".into(),
                    outcome: Outcome::Violated,
                },
            ],
        };
        assert!(!report.all_proven());
        assert_eq!(report.violated().len(), 1);
        assert!(report.unproven().is_empty());
    }

    /// A case proves one input only, so it must never make a report green.
    #[test]
    fn a_case_is_not_a_general_proof() {
        let report = check_source("#[case(0)]\n#[post(term == 0)]\nfn f(a: i64) -> i64 { 0 }\n")
            .expect("check");
        assert!(
            report
                .verdicts
                .iter()
                .any(|v| v.outcome == Outcome::ProvenForCase),
            "{:?}",
            report.verdicts
        );
        assert!(!report.all_proven());
    }

    #[test]
    fn uncontracted_source_yields_empty_report() {
        let report = check_source("fn plain(a: i64) -> i64 { a }\n").expect("check");
        assert!(report.verdicts.is_empty());
    }

    #[test]
    fn a_non_integer_return_type_is_rejected() {
        // A non-integer *parameter* is now admitted as opaque (it is never read);
        // the return is not, because `term` is compared as an integer.
        let err =
            check_source("#[pre(a >= 0)]\n#[post(term >= 0)]\nfn f(a: i64) -> String { 0 }\n")
                .expect_err("a non-integer return must be rejected");
        assert!(matches!(err, Error::Unsupported(_)), "{err}");
    }

    #[test]
    fn proven_contract_is_discharged_statically() {
        let report = check_source(
            "#[pre(a >= 0)]\n#[post(term == a + a)]\nfn double(a: i64) -> i64 { a + a }\n",
        )
        .expect("check");
        assert_eq!(report.verdicts[0].outcome, Outcome::Proven);
    }

    #[test]
    fn false_contract_is_caught_by_the_prover() {
        let report =
            check_source("#[pre(a >= 0)]\n#[post(term == 0)]\nfn lies(a: i64) -> i64 { 5 }\n")
                .expect("check");
        assert_eq!(report.verdicts[0].outcome, Outcome::Violated);
    }

    /// One unsupported function must not hide a provable one beside it.
    #[test]
    fn a_bad_function_does_not_mask_a_good_one() {
        let report = check_source(
            "#[post(term >= 0)]\nfn bad(a: i64) -> i64 { a[0] }\n\
             #[post(term >= 0)]\nfn good(a: i64) -> i64 { a }\n",
        )
        .expect("the good function should still be verified");
        assert_eq!(report.verdicts.len(), 1, "{:?}", report.verdicts);
        assert_eq!(report.verdicts[0].name, "good");
        assert!(
            report.skips.iter().any(|s| s.name == "bad"),
            "the rejected function must be a typed skip, not a silent drop; skips: {:?}",
            report.skips
        );
    }
}
