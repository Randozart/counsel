#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
#
# acceptance.sh — prove a Counsel build actually verifies what it claims.
#
#   ./scripts/acceptance.sh            # the working tree, by path dependency (pre-flight)
#   ./scripts/acceptance.sh 0.2.1      # the published crate from crates.io (post-publish)
#
# The cases are taken from `briev-counsel-lab/labs/kernels/`, not invented per
# release. That distinction is the whole point: 0.1.6 shipped loop invariants
# against a compiler that predated their fixes, and 0.2.0 shipped without call
# inlining. Both passed an acceptance that never touched the broken construct,
# because each acceptance was written from memory instead of from the kernels.
#
# A consumer gets a scratch project with no BRIEVC, no sibling checkout and no
# pre-seeded cache, so this measures the artefact rather than the working tree.
set -euo pipefail

cd "$(dirname "$0")/.."
crate_dir="$PWD"
version="${1:-}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

cargo new --lib "$work/proj" --vcs none >/dev/null
mkdir -p "$work/proj/tests"

if [ -n "$version" ]; then
    echo "==> acceptance: published counsel ${version}"
    ( cd "$work/proj" && cargo add "counsel@${version}" --features static >/dev/null 2>&1 )
else
    echo "==> acceptance: working tree at ${crate_dir}"
    printf '\ncounsel = { path = "%s", features = ["static"] }\n' "$crate_dir" \
        >> "$work/proj/Cargo.toml"
fi

cat > "$work/proj/tests/acceptance.rs" <<'RUST'
use counsel::check::{check_source, Outcome};

/// Verdicts for one source snippet, as (name, outcome) pairs.
fn outcomes(source: &str) -> Vec<(String, Outcome)> {
    check_source(source)
        .expect("acceptance source should be checkable")
        .verdicts
        .into_iter()
        .map(|verdict| (verdict.name, verdict.outcome))
        .collect()
}

fn all_proven(source: &str) -> Vec<(String, Outcome)> {
    let seen = outcomes(source);
    assert!(!seen.is_empty(), "expected verdicts, got none");
    assert!(
        seen.iter().all(|(_, outcome)| *outcome == Outcome::Proven),
        "not all proven: {seen:?}"
    );
    seen
}

/// labs/kernels/caller.rs — a call to a precondition-free helper resolves.
/// This is the case 0.2.0 shipped broken.
#[test]
fn call_to_a_pure_helper_is_resolved() {
    let source = "fn inc(a: i64) -> i64 { a + 1 }\n\
                  #[post(term == a + 2)]\n\
                  fn uses_inc(a: i64) -> i64 { inc(a) + 1 }\n";
    assert_eq!(outcomes(source), vec![("uses_inc".to_string(), Outcome::Proven)]);
}

/// labs/kernels/loop_invariant.rs — all three Hoare obligations.
#[test]
fn loop_invariants_prove_all_three_obligations() {
    let source = "#[pre(n >= 0 && n <= 1000)]\n\
                  #[post(term >= 0)]\n\
                  #[invariant(acc == i && i <= n)]\n\
                  fn count(n: i64) -> i64 { let mut acc = 0; for i in 0..n { acc = acc + 1; } acc }\n";
    let seen = all_proven(source);
    assert_eq!(seen.len(), 3, "{seen:?}");
}

/// labs/kernels/dynamic_loop_invariant.rs — a dynamic bound with a method tail.
#[test]
fn dynamic_bound_loop_with_a_method_tail() {
    let source = "#[post(term >= 1)]\n\
                  #[invariant(best >= 1)]\n\
                  fn optimal_step_cycle(len: usize) -> usize {\n\
                      let mut best = 1;\n\
                      let half = len / 2;\n\
                      for d in 1..=half { if len % d == 0 { best = d; } }\n\
                      best.max(1)\n\
                  }\n";
    let seen = all_proven(source);
    assert_eq!(seen.len(), 3, "{seen:?}");
}

/// labs/kernels/loop_sum.rs — a constant range unrolls.
#[test]
fn constant_bounded_loop_unrolls() {
    let source = "#[post(term == 3)]\n\
                  fn sum_three() -> i64 { let mut acc = 0; for i in 0..3 { acc = acc + i; } acc }\n";
    assert_eq!(outcomes(source), vec![("sum_three".to_string(), Outcome::Proven)]);
}

/// A false contract is still refuted — the check is not vacuous.
#[test]
fn a_false_contract_is_violated() {
    let source = "#[post(term == 0)]\nfn lies(a: i64) -> i64 { 5 }\n";
    assert_eq!(outcomes(source), vec![("lies".to_string(), Outcome::Violated)]);
}

/// A `#[case]` proves one input and must never make a report green.
#[test]
fn a_case_is_not_a_general_proof() {
    let source = "#[case(0)]\n#[post(term == 0)]\nfn f(a: i64) -> i64 { 0 }\n";
    let report = check_source(source).expect("case source should be checkable");
    assert!(
        report
            .verdicts
            .iter()
            .any(|verdict| verdict.outcome == Outcome::ProvenForCase),
        "{:?}",
        report.verdicts
    );
    assert!(!report.all_proven(), "a case must not pass as a general proof");
}
RUST

cd "$work/proj"
# A consumer has no compiler, no cache and no checkout: nothing to fall back on.
env -u BRIEVC -u COUNSEL_BRIEVC -u COUNSEL_BRIEVC_PATH -u COUNSEL_BRIEVC_CACHE_BIN \
    cargo test --quiet

echo "==> acceptance passed"
