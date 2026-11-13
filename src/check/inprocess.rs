// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// inprocess.rs — verify definitions in-process through the `briev-verify` library.
//
// This is the replacement for the compiler subprocess: the same questions are
// asked of the same prover, with no binary to locate, download or version-pin.

use super::build;
use super::{Error, FnVerdict, Outcome, Report};
use briev_verify::ast::TopLevel;
use briev_verify::verify::verify_bodies;

/// Whether a report entry names `name` — `"f: reason"` or a bare `"f"`.
fn mentions(entry: &str, name: &str) -> bool {
    entry == name || entry.starts_with(&format!("{name}: "))
}

/// The declaration name of a built item.
fn name_of(item: &TopLevel) -> String {
    match item {
        TopLevel::Definition(defn) => defn.name.clone(),
        _ => String::new(),
    }
}

/// Classify one built definition against the prover's report.
///
/// Note the last arm: a definition named nowhere was never examined, so it is
/// reported `Unproven`. Inferring `Proven` from absence is exactly the false
/// proof this crate must never ship.
fn outcome_of(name: &str, report: &briev_verify::verify::BodyReport) -> Outcome {
    if report.violated.iter().any(|entry| mentions(entry, name)) {
        return Outcome::Violated;
    }
    if report.unproven.iter().any(|entry| mentions(entry, name)) {
        return Outcome::Unproven;
    }
    if report.proven.iter().any(|proven| proven == name) {
        return Outcome::Proven;
    }
    Outcome::Unproven
}

/// Verify the contracted functions in a Rust source string, in-process.
///
/// Functions carrying neither `#[pre]` nor `#[post]` are skipped: only written
/// contracts are obligations.
pub fn check_source(source: &str) -> Result<Report, Error> {
    let file = syn::parse_file(source).map_err(|e| Error::Parse(e.to_string()))?;
    let items = build::definitions(&file)?;
    if items.is_empty() {
        return Ok(Report::default());
    }
    let report = verify_bodies(&items);
    let verdicts = items
        .iter()
        .map(|item| {
            let name = name_of(item);
            let outcome = outcome_of(&name, &report);
            FnVerdict { name, outcome }
        })
        .collect();
    Ok(Report {
        verdicts,
        warnings: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verdicts for one source snippet, in-process.
    fn verdicts(source: &str) -> Vec<(String, Outcome)> {
        check_source(source)
            .expect("in-process check")
            .verdicts
            .into_iter()
            .map(|v| (v.name, v.outcome))
            .collect()
    }

    /// A true identity is discharged with no compiler, no text and no process.
    #[test]
    fn proves_a_true_identity() {
        assert_eq!(
            verdicts("#[pre(a >= 0)]\n#[post(term == a + a)]\nfn double(a: i64) -> i64 { a + a }\n"),
            vec![("double".to_string(), Outcome::Proven)]
        );
    }

    /// A false postcondition is refuted, not silently accepted.
    ///
    /// The body is constant on purpose: a parameter-dependent contradiction
    /// (`a + a == 0` for `a >= 0`) is genuinely refutable only at a = 1, and a
    /// solver model with a free parameter stays advisory by design, so the honest
    /// verdict there is Unproven rather than Violated.
    #[test]
    fn refutes_a_false_contract() {
        assert_eq!(
            verdicts("#[post(term == 0)]\nfn lie(a: i64) -> i64 { 5 }\n"),
            vec![("lie".to_string(), Outcome::Violated)]
        );
    }

    /// A parameter-dependent contradiction is advisory, not a refutation.
    #[test]
    fn leaves_a_parameter_dependent_contradiction_advisory() {
        assert_eq!(
            verdicts("#[pre(a >= 0)]\n#[post(term == 0)]\nfn maybe(a: i64) -> i64 { a + a }\n"),
            vec![("maybe".to_string(), Outcome::Unproven)]
        );
    }

    /// Domain facts still pin a narrow width: `u8` cannot exceed 255, so a body
    /// that only holds because of the range is provable.
    #[test]
    fn carries_domain_facts_for_narrow_widths() {
        assert_eq!(
            verdicts("#[post(term <= 255)]\nfn fits(x: u8) -> u8 { x }\n"),
            vec![("fits".to_string(), Outcome::Proven)]
        );
    }

    /// Branches: the unguarded-else path must reach the right goal.
    #[test]
    fn proves_across_both_arms() {
        let source = "#[pre(a >= 0)]\n#[post(term >= 0)]\nfn abs_like(a: i64) -> i64 { if a < 0 { 0 - a } else { a } }\n";
        assert_eq!(verdicts(source), vec![("abs_like".to_string(), Outcome::Proven)]);
    }

    /// A supported method call desugars to the same guarded branches as before.
    #[test]
    fn proves_a_method_call_tail() {
        assert_eq!(
            verdicts("#[post(term >= 1)]\nfn at_least_one(x: i64) -> i64 { x.max(1) }\n"),
            vec![("at_least_one".to_string(), Outcome::Proven)]
        );
    }

    /// A declaration the prover never examines is Unproven, never Proven.
    #[test]
    fn an_unexamined_declaration_is_not_proven() {
        // A loop is not built in-process yet, so this must be an error rather
        // than a silent pass.
        let err = check_source("#[post(term >= 0)]\nfn f(n: i64) -> i64 { let mut a = 0; for i in 0..n { a = a + 1; } a }\n");
        assert!(err.is_err(), "an unbuilt construct must be refused, not proved");
    }
}
