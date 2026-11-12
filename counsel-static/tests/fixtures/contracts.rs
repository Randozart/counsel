// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// Fixture for the CLI smoke test. NOT compiled by cargo: files in a `tests/`
// subdirectory are not test targets, which is why contract attributes (unknown
// to rustc) are safe here.
//
// Expected `counsel-check` output: two verdicts, no violations, exit 0 —
// `double` advisory at the general goal and proven at its case, `sign` proven.

#[pre(a >= 0)]
#[post(term == 2 * a)]
#[case(5)]
fn double(a: i64) -> i64 {
    a + a
}

#[pre(a >= 0)]
#[post(term >= 0)]
fn sign(a: i64) -> i64 {
    a
}
