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
        // A case proves one concrete input, never the general contract.
        if name.contains("__case") {
            return Outcome::ProvenForCase;
        }
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
    // A single unsupported function must not hide the provable ones beside it, so
    // each is built independently and a rejection becomes a warning. A file whose
    // functions all fail still errors rather than reporting an empty pass.
    let mut items = Vec::new();
    let mut warnings = Vec::new();
    let mut first_error: Option<Error> = None;
    let mut contracted = 0usize;
    // Calls are resolved on the `syn` tree first, so the builder only ever sees
    // value expressions. Summaries are computed once for the whole file.
    let sums = build::build_summaries(&file.items);
    for item in &file.items {
        let syn::Item::Fn(item_fn) = item else {
            continue;
        };
        if !build::has_contract(item_fn) {
            continue;
        }
        contracted += 1;
        let inlined = match build::inline_calls(item_fn, &sums) {
            Ok(inlined) => inlined,
            Err(reason) => {
                warnings.push(format!("skipped: {reason}"));
                if first_error.is_none() {
                    first_error = Some(reason);
                }
                continue;
            }
        };
        match build::definition_for(&inlined) {
            Ok(found) => items.extend(found),
            Err(reason) => {
                warnings.push(format!("skipped: {reason}"));
                if first_error.is_none() {
                    first_error = Some(reason);
                }
            }
        }
    }
    if contracted > 0 && items.is_empty() {
        return Err(first_error.unwrap_or(Error::Unsupported(
            "no contracted function could be verified".to_string(),
        )));
    }
    if items.is_empty() {
        return Ok(Report::default());
    }
    let report = verify_bodies(&items);
    let verdicts = items
        .iter()
        .map(|item| {
            let name = name_of(item);
            // Classification uses the generated name; the report speaks about the
            // function the author wrote (`double__case0` is `double (case 0)`).
            let outcome = outcome_of(&name, &report);
            FnVerdict {
                name: super::display_name(&name),
                outcome,
            }
        })
        .collect();
    Ok(Report { verdicts, warnings })
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
            verdicts(
                "#[pre(a >= 0)]\n#[post(term == a + a)]\nfn double(a: i64) -> i64 { a + a }\n"
            ),
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
        assert_eq!(
            verdicts(source),
            vec![("abs_like".to_string(), Outcome::Proven)]
        );
    }

    /// A supported method call desugars to the same guarded branches as before.
    #[test]
    fn proves_a_method_call_tail() {
        assert_eq!(
            verdicts("#[post(term >= 1)]\nfn at_least_one(x: i64) -> i64 { x.max(1) }\n"),
            vec![("at_least_one".to_string(), Outcome::Proven)]
        );
    }

    /// A constant-bounded loop is unrolled and proved.
    #[test]
    fn proves_a_constant_bounded_loop() {
        let source = "#[post(term == 3)]\nfn sum_three() -> i64 { let mut acc = 0; for i in 0..3 { acc = acc + i; } acc }\n";
        assert_eq!(
            verdicts(source),
            vec![("sum_three".to_string(), Outcome::Proven)]
        );
    }

    /// A dynamic bound is refused rather than unrolled unsoundly.
    #[test]
    fn refuses_a_dynamic_loop_bound() {
        let err = check_source(
            "#[post(term >= 0)]\nfn f(n: i64) -> i64 { let mut a = 0; for i in 0..n { a = a + 1; } a }\n",
        )
        .expect_err("dynamic bounds must be refused");
        assert!(
            format!("{err}").contains("bounds must be integer literals"),
            "{err}"
        );
    }

    /// An invariant-carrying loop yields the three Hoare obligations, all proved.
    #[test]
    fn proves_all_three_loop_obligations() {
        let source = "#[pre(n >= 0 && n <= 1000)]\n#[post(term >= 0)]\n#[invariant(acc == i && i <= n)]\nfn count(n: i64) -> i64 { let mut acc = 0; for i in 0..n { acc = acc + 1; } acc }\n";
        let seen = verdicts(source);
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert!(seen.iter().all(|(_, o)| *o == Outcome::Proven), "{seen:?}");
    }

    /// A wrong invariant is caught at the obligation it actually breaks.
    #[test]
    fn a_wrong_invariant_fails_at_entry() {
        let source = "#[pre(n >= 0 && n <= 1000)]\n#[post(term >= 0)]\n#[invariant(acc == i + 1 && i <= n)]\nfn count_bad(n: i64) -> i64 { let mut acc = 0; for i in 0..n { acc = acc + 1; } acc }\n";
        let seen = verdicts(source);
        let init = seen
            .iter()
            .find(|(n, _)| n == "count_bad_init")
            .expect("init");
        assert_eq!(init.1, Outcome::Violated, "{seen:?}");
    }

    /// A loop whose tail is a method call uses the same desugaring.
    #[test]
    fn proves_a_loop_ending_in_a_method_call() {
        let source = "#[post(term >= 1)]\n#[invariant(best >= 1)]\nfn step(len: usize) -> usize { let mut best = 1; let half = len / 2; for d in 1..=half { if len % d == 0 { best = d; } } best.max(1) }\n";
        let seen = verdicts(source);
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert!(seen.iter().all(|(_, o)| *o == Outcome::Proven), "{seen:?}");
    }

    /// A call to a precondition-free helper resolves to the helper's value.
    ///
    /// The lab kernel `caller.rs` caught this missing after the text emitter was
    /// deleted: inlining lived there, and no test here exercised a call.
    #[test]
    fn resolves_a_call_to_a_pure_helper() {
        let source = "fn inc(a: i64) -> i64 { a + 1 }\n\
                      #[post(term == a + 2)]\n\
                      fn uses_inc(a: i64) -> i64 { inc(a) + 1 }\n";
        let seen = verdicts(source);
        assert_eq!(seen, vec![("uses_inc".to_string(), Outcome::Proven)]);
    }

    /// A callee with a precondition earns no summary, so its call is refused
    /// rather than assumed — the R3 rule that keeps the proof sound.
    #[test]
    fn refuses_a_call_to_a_preconditioned_helper() {
        let source = "#[pre(a > 0)]\nfn guarded(a: i64) -> i64 { a }\n\
                      #[post(term >= 0)]\nfn uses(a: i64) -> i64 { guarded(a) }\n";
        // Masking keeps the other declarations: the caller is skipped with a
        // warning rather than failing the whole file.
        let report = check_source(source).expect("the other function still verifies");
        assert!(
            report.verdicts.iter().all(|verdict| verdict.name != "uses"),
            "an unresolved call must never be reported: {:?}",
            report.verdicts
        );
        assert!(
            report.warnings.iter().any(|w| w.contains("a function call")),
            "{:?}",
            report.warnings
        );
    }

    /// A declaration the prover never examines is Unproven, never Proven.
    #[test]
    fn an_unexamined_declaration_is_not_proven() {
        // A loop is not built in-process yet, so this must be an error rather
        // than a silent pass.
        let err = check_source("#[post(term >= 0)]\nfn f(n: i64) -> i64 { let mut a = 0; for i in 0..n { a = a + 1; } a }\n");
        assert!(
            err.is_err(),
            "an unbuilt construct must be refused, not proved"
        );
    }
}
