// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// Counsel — static verification (the Phase 4 valve).
//
// Static pre/post-condition verification for a restricted pure-integer Rust
// subset, discharged through Briev's own body prover (`--verify-bodies`,
// briev-lang c0ce60dc). Declare contracts with plain attributes:
//
//   #[pre(a >= 0)]
//   #[post(term >= 0)]
//   fn double(a: i64) -> i64 { a * 2 }
//
// The function is translated to a Briev `defn` and proved statically — no
// runtime checks, no sampling, and no re-implementation of the prover here.
//
// Scope is deliberately narrow (i64 in/out, arithmetic, comparisons, `let`,
// `if`/`else`, `return`, tail expression). Everything else is rejected as
// unsupported: a wrong translation would be a wrong proof.

pub mod brievc;
pub mod translate;

use std::path::Path;

pub use brievc::Discharge;
pub use translate::TranslatedFn;

/// Why a verification attempt could not produce verdicts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The Rust source did not parse.
    Parse(String),
    /// The function used a construct outside the pure-integer subset.
    Unsupported(String),
    /// A contract declaration itself is malformed (for example a duplicated
    /// slot). Distinct from `Unsupported` so the message carries no subset
    /// hint that would mislead.
    Contract(String),
    /// No `brievc` with `--verify-bodies` was found.
    NoCompiler,
    /// The Briev compiler rejected the translated program.
    Compiler(String),
    /// Filesystem or process failure.
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
                 `if`/`else`, `return`, and a trailing expression"
            ),
            Error::Contract(why) => write!(f, "{why}"),
            Error::NoCompiler => write!(
                f,
                "no `brievc` with --verify-bodies found — set BRIEVC to a Briev \
                 compiler built from briev-lang c0ce60dc or later"
            ),
            Error::Compiler(why) => write!(f, "brievc failed:\n{why}"),
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

/// Verdicts for one source file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// One entry per contracted function, in source order.
    pub verdicts: Vec<FnVerdict>,
    /// Advisory messages: renamed or misspelt contract attributes. A warning
    /// never means "verified" — it means the attribute was not understood.
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
/// Functions without `#[pre]`/`#[post]` are ignored — only written
/// contracts are obligations.
pub fn check_source(source: &str) -> Result<Report, Error> {
    let file = syn::parse_file(source).map_err(|e| Error::Parse(e.to_string()))?;
    // A single unsupported function must not hide the others. `translate_file`
    // is all-or-nothing (callers that want strictness keep using it), so on
    // failure retry one function at a time and record each reject as a warning.
    // Without this, one expressive body masked every provable body beside it.
    let (translated, warnings) = match translate::translate_file(&file) {
        Ok(pair) => pair,
        Err(first) => translate_each(&file, first)?,
    };
    let discharge = brievc::discharge(&translated)?;
    let mut report = assemble(&translated, &discharge);
    report.warnings = warnings;
    Ok(report)
}

/// Translate function by function, recording rejects instead of aborting.
///
/// Returns the original error when nothing at all could be translated, so a
/// wholly unsupported file still fails rather than reporting an empty pass.
fn translate_each(
    file: &syn::File,
    first: Error,
) -> Result<(Vec<TranslatedFn>, Vec<String>), Error> {
    let mut defns = Vec::new();
    let mut warnings = Vec::new();
    for item in &file.items {
        if !matches!(item, syn::Item::Fn(_)) {
            continue;
        }
        let single = syn::File {
            shebang: None,
            attrs: Vec::new(),
            items: vec![item.clone()],
        };
        match translate::translate_file(&single) {
            Ok((found, found_warnings)) => {
                defns.extend(found);
                warnings.extend(found_warnings);
            }
            Err(reason) => warnings.push(format!("skipped: {reason}")),
        }
    }
    if defns.is_empty() {
        return Err(first);
    }
    Ok((defns, warnings))
}

/// Verify the contracted functions in a Rust file.
pub fn check_file(path: &Path) -> Result<Report, Error> {
    let source =
        std::fs::read_to_string(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    check_source(&source)
}

/// Merge discharge results back onto the translated declarations.
fn assemble(translated: &[TranslatedFn], discharge: &Discharge) -> Report {
    let verdicts = translated
        .iter()
        .map(|decl| {
            let outcome = classify(&decl.name, discharge);
            FnVerdict {
                name: display_name(&decl.name),
                outcome,
            }
        })
        .collect();
    Report {
        verdicts,
        warnings: Vec::new(),
    }
}

/// Readable name for a verdict: `<fn>__caseN` becomes `<fn> (case N)`.
///
/// The generated defn name keeps the case distinguishable while checking; the
/// report should speak about the function the author wrote.
fn display_name(name: &str) -> String {
    match name.split_once("__case") {
        Some((base, index)) => format!("{base} (case {index})"),
        None => name.to_string(),
    }
}

/// Classify one declaration from the discharge report.
fn classify(name: &str, discharge: &Discharge) -> Outcome {
    if discharge.violated.iter().any(|n| n == name) {
        return Outcome::Violated;
    }
    if discharge.unproven.iter().any(|n| n == name) {
        return Outcome::Unproven;
    }
    // A case defn is named `<fn>__caseN` and proves one concrete input only.
    if name.contains("__case") {
        return Outcome::ProvenForCase;
    }
    Outcome::Proven
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether a usable compiler is present for the integration-level tests.
    fn have_compiler() -> bool {
        brievc::find_compiler().is_some()
    }

    #[test]
    fn classify_uses_the_three_way_report() {
        let discharge = Discharge {
            proven: vec!["a".into()],
            unproven: vec!["b".into()],
            violated: vec!["c".into()],
            names_authoritative: false,
        };
        assert_eq!(classify("a", &discharge), Outcome::Proven);
        assert_eq!(classify("b", &discharge), Outcome::Unproven);
        assert_eq!(classify("c", &discharge), Outcome::Violated);
    }

    #[test]
    fn report_helpers_split_outcomes() {
        let report = Report {
            warnings: Vec::new(),
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

    #[test]
    fn uncontracted_source_yields_empty_report() {
        let report = check_source("fn plain(a: i64) -> i64 { a }\n").expect("check");
        assert!(report.verdicts.is_empty());
    }

    #[test]
    fn wrong_boundary_type_is_reported_without_running_the_compiler() {
        let err = check_source("#[post(true)]\nfn f(a: u32) -> i64 { 0 }\n")
            .expect_err("u32 must be rejected");
        assert!(matches!(err, Error::Unsupported(_)), "{err}");
    }

    #[test]
    fn proven_contract_is_discharged_statically() {
        if !have_compiler() {
            return;
        }
        let report = check_source(
            "#[pre(a >= 0)]\n#[post(term == a + a)]\nfn double(a: i64) -> i64 { a + a }\n",
        )
        .expect("check");
        assert_eq!(report.verdicts[0].outcome, Outcome::Proven);
    }

    #[test]
    fn false_contract_is_caught_by_the_prover() {
        if !have_compiler() {
            return;
        }
        let report =
            check_source("#[pre(a >= 0)]\n#[post(term == 0)]\nfn lies(a: i64) -> i64 { 5 }\n")
                .expect("check");
        assert_eq!(report.verdicts[0].outcome, Outcome::Violated);
    }
}

#[cfg(test)]
mod masking_tests {
    use super::*;

    /// One unsupported function must not hide a translatable one beside it.
    ///
    /// `translate_file` stays all-or-nothing (its callers want strictness); the
    /// resilience lives in `check_source`, which retries per function.
    #[test]
    fn a_bad_function_does_not_mask_a_good_one() {
        let file = syn::parse_file(
            "#[post(term >= 0)]\nfn bad(a: i64) -> i64 { a[0] }\n\
             #[post(term >= 0)]\nfn good(a: i64) -> i64 { a }\n",
        )
        .expect("parse");
        let first =
            translate::translate_file(&file).expect_err("file-level translation is all-or-nothing");
        let (defns, warnings) = translate_each(&file, first).expect("per-function retry");
        assert_eq!(
            defns.len(),
            1,
            "the good function should survive: {warnings:?}"
        );
        assert_eq!(defns[0].name, "good");
        assert!(
            warnings.iter().any(|warning| warning.contains("skipped")),
            "the skip must be reported, not silent: {warnings:?}"
        );
    }
}
