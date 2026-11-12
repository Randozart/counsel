// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// `counsel-check <file.rs>` — run the static valve over one Rust file.
//
// The library is the API; this binary is the shortest path from a file on disk
// to a verdict. Exit status: 0 when nothing is violated, 1 when any contract is
// falsified, 2 when the check could not run at all (bad usage, unparsable
// source, or no usable brievc).

use std::process::ExitCode;

/// Run the static check for one path, printing verdicts and warnings.
fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: counsel-check <file.rs>");
        return ExitCode::from(2);
    };
    let report = match counsel_static::check_file(std::path::Path::new(&path)) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(2);
        }
    };
    print_report(&report);
    if report.violated().is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Print warnings first (they explain missing verdicts), then the verdicts.
fn print_report(report: &counsel_static::Report) {
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    if report.verdicts.is_empty() {
        println!("no contracted functions found");
        return;
    }
    for verdict in &report.verdicts {
        println!("{:<32} {}", verdict.name, label(&verdict.outcome));
    }
    // A case proves one input, never the function: say so rather than let the
    // reader infer a general proof.
    let cases = report
        .verdicts
        .iter()
        .filter(|v| v.outcome == counsel_static::Outcome::ProvenForCase)
        .count();
    if cases > 0 {
        println!("\n{cases} case(s) proved at their own inputs only — not a general proof");
    }
}

/// Human wording for a verdict.
fn label(outcome: &counsel_static::Outcome) -> &'static str {
    match outcome {
        counsel_static::Outcome::Proven => "proven",
        counsel_static::Outcome::ProvenForCase => "proven (case only)",
        counsel_static::Outcome::Unproven => "unproven (advisory)",
        counsel_static::Outcome::Violated => "VIOLATED",
    }
}
