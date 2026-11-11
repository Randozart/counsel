// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// src/brievc.rs — locate the Briev compiler and discharge translated contracts.
//
// The valve reuses Briev's own prover: the translated `defn`s are written to a
// temporary `.bv` file and compiled with `brievc build <file> --library
// --verify-bodies`. That gate fails on a concrete counterexample (error) and
// reports undecidable bodies as advisory warnings, which maps directly onto
// this crate's Violated / Unproven / Proven outcomes.

use crate::translate::TranslatedFn;
use crate::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The capability flag this crate depends on (briev-lang c0ce60dc).
const FLAG: &str = "--verify-bodies";

/// Per-declaration discharge results, keyed by function name.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Discharge {
    /// Names whose contracts the prover discharged.
    pub proven: Vec<String>,
    /// Names the prover could not decide (advisory, not failures).
    pub unproven: Vec<String>,
    /// Names with a concrete counterexample.
    pub violated: Vec<String>,
}

/// Locate a `brievc` binary that supports `--verify-bodies`.
///
/// Candidates are tried in order: `$BRIEVC`, `brievc` on PATH, then the
/// sibling `briev-lang` builds. A candidate is accepted only if its own help
/// output advertises the capability flag — a stale compiler without the flag
/// would otherwise fail the discharge with a confusing usage error.
pub fn find_compiler() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(explicit) = std::env::var("BRIEVC") {
        candidates.push(PathBuf::from(explicit));
    }
    candidates.push(PathBuf::from("brievc"));
    candidates.extend(sibling_builds());
    candidates
        .into_iter()
        .find(|candidate| supports_flag(candidate))
}

/// Candidate compiler paths from a sibling `briev-lang` checkout.
fn sibling_builds() -> Vec<PathBuf> {
    let mut out = Vec::new();
    // Walk up from this crate to the projects root, then look for briev-lang.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for ancestor in manifest.ancestors().skip(1).take(4) {
        let base = ancestor.join("briev-lang").join("target");
        out.push(base.join("debug").join("brievc"));
        out.push(base.join("release").join("brievc"));
    }
    out
}

/// Whether a candidate binary advertises the `--verify-bodies` capability.
fn supports_flag(candidate: &Path) -> bool {
    let Ok(output) = Command::new(candidate).arg("--help").output() else {
        return false;
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    text.contains("verify-bodies")
}

/// Discharge every translated declaration through `brievc --verify-bodies`.
pub fn discharge(defns: &[TranslatedFn]) -> Result<Discharge, Error> {
    if defns.is_empty() {
        return Ok(Discharge::default());
    }
    let compiler = find_compiler().ok_or(Error::NoCompiler)?;
    let workdir = temp_workdir();
    std::fs::create_dir_all(&workdir)
        .map_err(|e| Error::Io(format!("{}: {e}", workdir.display())))?;
    let result = run_discharge(&compiler, &workdir, defns);
    let _ = std::fs::remove_dir_all(&workdir);
    result
}

/// Write the pool of definitions and run the compiler over it.
fn run_discharge(
    compiler: &Path,
    workdir: &Path,
    defns: &[TranslatedFn],
) -> Result<Discharge, Error> {
    let source = workdir.join("counsel_static.bv");
    let pool: String = defns.iter().map(|d| d.briev.as_str()).collect();
    std::fs::write(&source, pool).map_err(|e| Error::Io(format!("{}: {e}", source.display())))?;
    let output = Command::new(compiler)
        .arg("build")
        .arg(&source)
        .arg("--library")
        .arg(FLAG)
        .arg("--out")
        .arg(workdir.join("out"))
        .output()
        .map_err(|e| Error::Io(format!("failed to run brievc: {e}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let mut discharge = parse_report(&stderr);
    if !output.status.success() && discharge.violated.is_empty() {
        // A compiler-side failure (parse/type error, or a prover crash) is not
        // a contract verdict — surface it rather than inventing an outcome.
        return Err(Error::Compiler(tail(&stderr, 12)));
    }
    discharge.proven = proven_names(defns, &discharge);
    Ok(discharge)
}

/// Parse the three-way verdict out of the compiler's stderr.
///
/// Violations arrive as a block introduced by `error: --verify-bodies:` with
/// one `<name>: body can violate ...` line per declaration; undecidables
/// arrive as `warning: --verify-bodies could not prove: <name>: ...`.
fn parse_report(stderr: &str) -> Discharge {
    let mut discharge = Discharge::default();
    let mut in_violations = false;
    for line in stderr.lines() {
        let line = line.trim();
        if line.starts_with("error: --verify-bodies:") {
            in_violations = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("warning: --verify-bodies could not prove: ") {
            if let Some(name) = declaration_name(rest) {
                discharge.unproven.push(name);
            }
            continue;
        }
        if in_violations {
            if let Some(name) = declaration_name(line) {
                discharge.violated.push(name);
            }
        }
    }
    discharge
}

/// The declaration name prefix of a report line (`name: reason`).
fn declaration_name(line: &str) -> Option<String> {
    let (name, _) = line.split_once(':')?;
    let name = name.trim();
    if name.is_empty() || name.contains(' ') {
        return None;
    }
    Some(name.to_string())
}

/// Names not reported as violated or unproven were proven by the gate.
fn proven_names(defns: &[TranslatedFn], discharge: &Discharge) -> Vec<String> {
    defns
        .iter()
        .map(|d| &d.name)
        .filter(|name| !discharge.violated.contains(name) && !discharge.unproven.contains(name))
        .cloned()
        .collect()
}

/// Last `n` lines of compiler output, for the error message.
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// A unique, deterministic-enough scratch directory under the system temp dir.
fn temp_workdir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("counsel-static-{}-{nanos}", std::process::id()))
}
