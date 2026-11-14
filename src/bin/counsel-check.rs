// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// `counsel-check [--format human|json] <file.rs | directory>`
//
// The library is the API; this binary is the shortest path from source on disk
// to verdicts. A directory is walked for `.rs` files.
//
// Exit status: 0 when nothing is violated, 1 when a contract is falsified, 2
// when the check could not run (bad usage, unparsable source, no usable brievc).

use counsel::check::{Outcome, Report};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Output shape for the verdicts.
#[derive(Clone, Copy, PartialEq)]
enum Format {
    Human,
    Json,
}

/// Check the target(s), print, and map the outcome to an exit status.
fn main() -> ExitCode {
    let (format, target) = match parse_args() {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let files = match rust_files(&target) {
        Ok(files) if !files.is_empty() => files,
        Ok(_) => {
            eprintln!("no .rs files under {}", target.display());
            return ExitCode::from(2);
        }
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let mut reports: Vec<(PathBuf, Report)> = Vec::new();
    for file in files {
        match counsel::check::check_file(&file) {
            Ok(report) => reports.push((file, report)),
            Err(err) => {
                eprintln!("error: {}: {err}", file.display());
                return ExitCode::from(2);
            }
        }
    }
    match format {
        Format::Json => print_json(&reports),
        Format::Human => print_human(&reports),
    }
    if reports
        .iter()
        .any(|(_, report)| !report.violated().is_empty())
    {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Parse `[--format human|json] <target>`.
fn parse_args() -> Result<(Format, PathBuf), String> {
    let mut format = Format::Human;
    let mut target: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--format" {
            format = match args.next().as_deref() {
                Some("json") => Format::Json,
                Some("human") => Format::Human,
                Some(other) => {
                    return Err(format!(
                        "unknown --format '{other}' (expected human or json)"
                    ))
                }
                None => return Err("--format requires a value".to_string()),
            };
            continue;
        }
        target = Some(PathBuf::from(arg));
    }
    target.map(|path| (format, path)).ok_or_else(|| {
        "usage: counsel-check [--format human|json] <file.rs | directory>".to_string()
    })
}

/// The Rust files to check: the path itself, or every `.rs` beneath it.
fn rust_files(target: &Path) -> Result<Vec<PathBuf>, String> {
    if target.is_file() {
        return Ok(vec![target.to_path_buf()]);
    }
    if !target.is_dir() {
        return Err(format!("{} is not a file or directory", target.display()));
    }
    let mut found = Vec::new();
    let mut stack = vec![target.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Human-readable output: warnings first, then one line per verdict.
fn print_human(reports: &[(PathBuf, Report)]) {
    let multiple = reports.len() > 1;
    let mut cases = 0usize;
    for (path, report) in reports {
        if multiple {
            println!("{}:", path.display());
        }
        for warning in &report.warnings {
            eprintln!("warning: {warning}");
        }
        // One file: show every skip. A whole tree: a count, because a census of
        // 12k functions would otherwise be unreadable — use --format json for detail.
        if !multiple {
            for skip in &report.skips {
                match &skip.body {
                    None => eprintln!(
                        "not checked: {}: {} (body is in subset)",
                        skip.name, skip.reason
                    ),
                    Some(reason) => eprintln!(
                        "not checked: {}: {} — body: {}",
                        skip.name, skip.reason, reason
                    ),
                }
            }
        } else if !report.skips.is_empty() {
            println!("  {} not checked", report.skips.len());
        }
        for verdict in &report.verdicts {
            if verdict.outcome == Outcome::ProvenForCase {
                cases += 1;
            }
            let indent = if multiple { "  " } else { "" };
            println!("{indent}{:<32} {}", verdict.name, label(&verdict.outcome));
        }
    }
    // A case proves one input, never the function: say so rather than let the
    // reader infer a general proof.
    if cases > 0 {
        println!("\n{cases} case(s) proved at their own inputs only — not a general proof");
    }
}

/// Machine-readable output, for editors and CI.
fn print_json(reports: &[(PathBuf, Report)]) {
    let files: Vec<serde_json::Value> = reports
        .iter()
        .map(|(path, report)| {
            let verdicts: Vec<serde_json::Value> = report
                .verdicts
                .iter()
                .map(|verdict| {
                    serde_json::json!({
                        "name": verdict.name,
                        "outcome": json_label(&verdict.outcome),
                    })
                })
                .collect();
            serde_json::json!({
                "path": path.display().to_string(),
                "warnings": report.warnings,
                "verdicts": verdicts,
                "skips": report
                    .skips
                    .iter()
                    .map(|skip| serde_json::json!({
                        "name": skip.name,
                        "reason": skip.reason,
                        "body": skip.body,
                    }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    println!("{}", serde_json::json!({ "files": files }));
}

/// Stable identifier for a verdict, for machine consumption.
fn json_label(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Proven => "proven",
        Outcome::ProvenForCase => "proven-for-case",
        Outcome::Unproven => "unproven",
        Outcome::Violated => "violated",
    }
}

/// Human wording for a verdict.
fn label(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Proven => "proven",
        Outcome::ProvenForCase => "proven (case only)",
        Outcome::Unproven => "unproven (advisory)",
        Outcome::Violated => "VIOLATED",
    }
}
