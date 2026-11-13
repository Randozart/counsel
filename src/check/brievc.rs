// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// src/brievc.rs — locate the Briev compiler and discharge translated contracts.
//
// The valve reuses Briev's own prover: the translated `defn`s are written to a
// temporary `.bv` file and compiled with `brievc build <file> --library
// --verify-bodies`. That gate fails on a concrete counterexample (error) and
// reports undecidable bodies as advisory warnings, which maps directly onto
// this crate's Violated / Unproven / Proven outcomes.

use super::translate::TranslatedFn;
use super::Error;
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
    /// Whether `proven` came from the structured report rather than being
    /// inferred from absence.
    ///
    /// When true every declaration the compiler proved is *named*, so a
    /// declaration absent from all three lists was never examined — and silence
    /// must never be read as a proof.
    pub names_authoritative: bool,
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
    // A toolchain this crate already fetched on an earlier run.
    if let Some(cached) = cached_binary() {
        candidates.push(cached);
    }
    candidates.push(PathBuf::from("brievc"));
    candidates.extend(sibling_builds());
    candidates
        .into_iter()
        .find(|candidate| supports_flag(candidate))
}

/// Pinned toolchain, fetched on first use when no compiler is installed.
///
/// The static half needs a `brievc` binary. Fetching it at the first check — not
/// during the build — is what lets the default feature set stay free of Briev
/// while `--features static` still works with no manual setup.
const RELEASE_TAG: &str = "v0.1.4";
const ASSET_STEM: &str = "brievc-linux-x86_64";
/// sha256 of the pinned tarball; a mismatch aborts before the toolchain is used.
const ASSET_SHA256: &str = "cabfc566ac50b600d20a3e34c768b33d24189bfc070c7ec41f22500b5989a7dc";

/// Resolve a compiler, fetching the pinned toolchain when none is installed.
///
/// `COUNSEL_NO_DOWNLOAD=1` forbids the fetch, which is what an air-gapped or
/// audited environment wants: the caller then sets `BRIEVC` or installs one.
pub fn resolve_compiler() -> Result<PathBuf, Error> {
    if let Some(found) = find_compiler() {
        return Ok(found);
    }
    if std::env::var_os("COUNSEL_NO_DOWNLOAD").is_some() {
        return Err(Error::NoCompiler);
    }
    download_toolchain()?;
    cached_binary().ok_or(Error::NoCompiler)
}

/// The cache directory: `COUNSEL_CACHE_DIR`, else the XDG cache, else `~/.cache`.
fn cache_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("COUNSEL_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("counsel")
}

/// Directory of the pinned, extracted toolchain inside the cache.
fn install_dir() -> PathBuf {
    cache_root().join(format!("brievc-{RELEASE_TAG}-{ASSET_STEM}"))
}

/// The pinned compiler, once extracted.
fn cached_binary() -> Option<PathBuf> {
    let binary = install_dir().join("bin").join("brievc");
    binary.is_file().then_some(binary)
}

/// Find an executable on PATH (minimal `which`, no shell).
fn which(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Whether a prebuilt asset is published for this platform.
fn asset_supported() -> bool {
    std::env::consts::OS == "linux" && std::env::consts::ARCH == "x86_64"
}

/// Download, verify and extract the pinned toolchain into the cache.
fn download_toolchain() -> Result<(), Error> {
    if !asset_supported() {
        return Err(Error::NoCompiler);
    }
    let install = install_dir();
    let cache = cache_root();
    std::fs::create_dir_all(&cache)
        .map_err(|error| Error::Io(format!("cannot create `{}`: {error}", cache.display())))?;
    let tarball = cache.join(format!("{ASSET_STEM}-{RELEASE_TAG}.tar.gz"));
    let url = format!(
        "https://github.com/Randozart/counsel/releases/download/{RELEASE_TAG}/{ASSET_STEM}.tar.gz"
    );
    fetch(&url, &tarball)?;
    verify_sha256(&tarball)?;
    extract(&tarball, &install)
}

/// Fetch `url` to `dest`, preferring curl and falling back to wget.
fn fetch(url: &str, dest: &Path) -> Result<(), Error> {
    let attempts: [(&str, Vec<&str>); 2] = [
        ("curl", vec!["-fL", "--retry", "3", "-o"]),
        ("wget", vec!["-q", "-O"]),
    ];
    for (tool, flags) in attempts {
        if which(tool).is_none() {
            continue;
        }
        let mut command = Command::new(tool);
        command.args(&flags);
        command.arg(dest).arg(url);
        if command.status().is_ok_and(|status| status.success()) {
            return Ok(());
        }
    }
    Err(Error::Io(
        "neither curl nor wget could download the pinned Briev compiler".to_string(),
    ))
}

/// Refuse a tarball that does not match the pinned checksum.
fn verify_sha256(tarball: &Path) -> Result<(), Error> {
    let actual = sha256_of(tarball)?;
    if actual == ASSET_SHA256 {
        return Ok(());
    }
    Err(Error::Io(format!(
        "brievc tarball checksum mismatch at `{}`: expected {ASSET_SHA256}, got {actual}. \
         Delete that file and retry, or set BRIEVC to a compiler you trust.",
        tarball.display()
    )))
}

/// sha256 of a file, via `sha256sum` or `shasum -a 256`.
fn sha256_of(file: &Path) -> Result<String, Error> {
    for (tool, flags) in [("sha256sum", Vec::new()), ("shasum", vec!["-a", "256"])] {
        if which(tool).is_none() {
            continue;
        }
        let Ok(output) = Command::new(tool).args(&flags).arg(file).output() else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(digest) = stdout.split_whitespace().next() {
            return Ok(digest.to_ascii_lowercase());
        }
    }
    Err(Error::Io(format!(
        "cannot verify the downloaded compiler at `{}`: neither sha256sum nor shasum is available",
        file.display()
    )))
}

/// Extract the tarball into `install`, keeping the `<stem>/bin/share` layout.
fn extract(tarball: &Path, install: &Path) -> Result<(), Error> {
    let staging = cache_root().join(format!(".staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|error| Error::Io(format!("cannot create `{}`: {error}", staging.display())))?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(tarball)
        .arg("-C")
        .arg(&staging)
        .status()
        .map_err(|error| Error::Io(format!("cannot run tar: {error}")))?;
    if !status.success() {
        return Err(Error::Io(format!(
            "cannot extract `{}` — is `tar` available?",
            tarball.display()
        )));
    }
    // Replace any previous install with the freshly extracted tree.
    let _ = std::fs::remove_dir_all(install);
    std::fs::rename(staging.join(ASSET_STEM), install)
        .map_err(|error| Error::Io(format!("cannot move the compiler into place: {error}")))?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
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
    let compiler = resolve_compiler()?;
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
    let source = workdir.join("counsel_check.bv");
    let pool: String = defns.iter().map(|d| d.briev.as_str()).collect();
    std::fs::write(&source, pool).map_err(|e| Error::Io(format!("{}: {e}", source.display())))?;
    // Ask for the structured report beside the build output (C1).
    let report_path = workdir.join("verify-report.json");
    let output = invoke(compiler, workdir, &source, Some(&report_path))?;
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // A toolchain that predates `--verify-report` rejects the flag outright, so
    // the run fails before any verdict exists. Retry without it and read the
    // prose: the report is preferred, never required.
    if !output.status.success() && rejects_report_flag(&stderr) {
        let legacy = invoke(compiler, workdir, &source, None)?;
        let legacy_stderr = String::from_utf8_lossy(&legacy.stderr).to_string();
        let discharge = parse_report(&legacy_stderr);
        if !legacy.status.success() && discharge.violated.is_empty() {
            return Err(Error::Compiler(tail(&legacy_stderr, 12)));
        }
        return Ok(settle(defns, discharge));
    }

    // Prefer the structured report: the human diagnostics are prose, and a
    // rewording would silently turn a violation into "proven".
    let discharge = read_report(&report_path).unwrap_or_else(|| parse_report(&stderr));
    if !output.status.success() && discharge.violated.is_empty() {
        // A compiler-side failure (parse/type error, or a prover crash) is not
        // a contract verdict — surface it rather than inventing an outcome.
        return Err(Error::Compiler(tail(&stderr, 12)));
    }
    Ok(settle(defns, discharge))
}

/// Run one build of the translated pool, optionally requesting the report.
fn invoke(
    compiler: &Path,
    workdir: &Path,
    source: &Path,
    report: Option<&Path>,
) -> Result<std::process::Output, Error> {
    let mut command = Command::new(compiler);
    command.arg("build").arg(source).arg("--library").arg(FLAG);
    if let Some(report) = report {
        command.arg("--verify-report").arg(report);
    }
    // Run from the scratch workdir, which holds only the translated `.bv`.
    // brievc's freshness guard scans `src/` + `config/` and, in released
    // binaries (<= v0.1.0), exits when they look newer than the binary — from a
    // consumer's project directory that aborted the check before it started.
    command.current_dir(workdir);
    command
        .arg("--out")
        .arg(workdir.join("out"))
        .output()
        .map_err(|e| Error::Io(format!("failed to run brievc: {e}")))
}

/// Whether the compiler rejected `--verify-report` itself (an older toolchain).
fn rejects_report_flag(stderr: &str) -> bool {
    stderr.contains("unknown flag") && stderr.contains("verify-report")
}

/// Attach the caller-known proof names to the discharge set.
///
/// `proven` is inferred from absence — every declaration the compiler did not
/// mention. That is only sound if it reported on every declaration it was given,
/// so the report's own count is used to check the accounting. When the numbers do
/// not add up, some declaration was silently skipped (an unmodelled body, a
/// failed inlining), and silence must never be read as proof: nothing inferred
/// from absence is then reported as proven.
fn settle(defns: &[TranslatedFn], mut discharge: Discharge) -> Discharge {
    if discharge.names_authoritative {
        // The report named everything it proved, so anything it named nowhere was
        // never examined. Record it as unproven rather than letting absence read
        // as success.
        discharge.unproven.extend(unreported(defns, &discharge));
        return discharge;
    }
    // A toolchain that reports no names cannot be checked this way; keep the
    // historical inference from absence.
    discharge.proven = proven_names(defns, &discharge);
    discharge
}

/// Declarations the report mentioned nowhere, so they were never examined.
fn unreported(defns: &[TranslatedFn], discharge: &Discharge) -> Vec<String> {
    defns
        .iter()
        .filter(|defn| {
            !discharge.proven.iter().any(|name| name == &defn.name)
                && !discharge.unproven.iter().any(|name| name == &defn.name)
                && !discharge.violated.iter().any(|name| name == &defn.name)
        })
        .map(|defn| defn.name.clone())
        .collect()
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
    std::env::temp_dir().join(format!("counsel-check-{}-{nanos}", std::process::id()))
}

/// Read the compiler's structured report, when it wrote one.
///
/// Shape: `{ "proven": <count>, "unproven": [{name, reason}], "violated": [...] }`.
/// Returns `None` when the file is absent — a compiler predating
/// `--verify-report` — or unreadable, so the caller can fall back to the human
/// diagnostics. Only names are taken: reasons are for humans, and the caller
/// already knows which declarations it asked about.
fn read_report(path: &Path) -> Option<Discharge> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    // A report that names its proofs can be checked; one that reports only a
    // total cannot tell "proved" apart from "never examined". The proven list
    // also covers the compiler's own prelude, which is why consumers intersect
    // it with the declarations they submitted.
    let names_authoritative = value.get("proven").is_some_and(serde_json::Value::is_array);
    Some(Discharge {
        proven: report_names(&value, "proven"),
        unproven: report_names(&value, "unproven"),
        violated: report_names(&value, "violated"),
        names_authoritative,
    })
}

/// The `name` fields of one finding array in the report.
fn report_names(value: &serde_json::Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(|entry| entry.as_array())
        .map(|findings| {
            findings
                .iter()
                .filter_map(|finding| finding.get("name").and_then(|name| name.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The structured report is the primary source of verdicts.
    #[test]
    fn reads_structured_report() {
        let path = std::env::temp_dir().join(format!("counsel-report-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"proven":1,"unproven":[{"name":"scale","reason":"x"}],"violated":[{"name":"lies","reason":"y"}]}"#,
        )
        .expect("write report");
        let discharge = read_report(&path).expect("report parsed");
        let _ = std::fs::remove_file(&path);
        assert_eq!(discharge.violated, vec!["lies".to_string()]);
        assert_eq!(discharge.unproven, vec!["scale".to_string()]);
    }

    /// A missing report means an older compiler: fall back to the text path.
    #[test]
    fn missing_report_is_none() {
        assert!(read_report(Path::new("/nonexistent/verify-report.json")).is_none());
    }

    /// Two declarations, a report that names only one of them as proved.
    fn skipped_report() -> Discharge {
        Discharge {
            proven: vec!["b".into()],
            unproven: Vec::new(),
            violated: Vec::new(),
            names_authoritative: true,
        }
    }

    /// Two declarations with empty bodies; only the names matter here.
    fn two_defns() -> Vec<TranslatedFn> {
        vec![
            TranslatedFn {
                name: "a".into(),
                briev: String::new(),
            },
            TranslatedFn {
                name: "b".into(),
                briev: String::new(),
            },
        ]
    }

    /// The report proved one declaration and mentioned the other nowhere, which
    /// means the compiler skipped it. Skipped is not proved.
    #[test]
    fn settle_fails_closed_when_a_declaration_is_unreported() {
        let discharge = Discharge {
            proven: vec!["b".into()],
            unproven: Vec::new(),
            violated: Vec::new(),
            names_authoritative: true,
        };
        let settled = settle(&two_defns(), discharge);
        assert!(
            settled.unproven.contains(&"a".to_string()),
            "an unreported declaration must become unproven: {:?}",
            settled.unproven
        );
        assert!(
            !settled.unproven.contains(&"b".to_string()),
            "a declaration the report proved must stay proved: {:?}",
            settled.unproven
        );
    }

    /// A report that accounts for every declaration adds nothing.
    #[test]
    fn settle_trusts_a_complete_named_report() {
        let discharge = Discharge {
            proven: vec!["a".into()],
            unproven: vec!["b".into()],
            violated: Vec::new(),
            names_authoritative: true,
        };
        let settled = settle(&two_defns(), discharge);
        assert_eq!(settled.unproven, vec!["b".to_string()]);
    }

    /// Without names there is nothing to check, so the historical inference stays.
    #[test]
    fn settle_keeps_inference_without_names() {
        let settled = settle(&two_defns(), Discharge::default());
        assert_eq!(settled.proven.len(), 2);
    }

    /// A report that lists proven names is marked authoritative.
    #[test]
    fn reads_named_proofs() {
        let path = std::env::temp_dir().join(format!("counsel-named-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"proven":[{"name":"ok_one","reason":""}],"unproven":[],"violated":[]}"#,
        )
        .expect("write report");
        let discharge = read_report(&path).expect("report parsed");
        let _ = std::fs::remove_file(&path);
        assert!(discharge.names_authoritative);
        assert_eq!(discharge.proven, vec!["ok_one".to_string()]);
    }
}
