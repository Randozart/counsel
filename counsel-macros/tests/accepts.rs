// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
//! rustc must accept the contract attributes, and they must be inert.
//!
//! This is the adoption test: without these macros a real crate cannot even
//! compile a function carrying `#[pre]`, because rustc rejects unknown
//! attributes. `counsel-static` still reads the source text — the macros only
//! make the syntax legal.

use counsel_macros::{case, post, pre};

#[pre(a >= 0)]
#[post(term == a + a)]
#[case(5)]
fn double(a: i64) -> i64 {
    a + a
}

#[test]
fn attributes_are_accepted_and_inert() {
    // The attributes contribute no runtime behaviour whatsoever.
    assert_eq!(double(5), 10);
    assert_eq!(double(-3), -6);
}
