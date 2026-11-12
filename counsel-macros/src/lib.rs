// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
//! Contract attributes that rustc accepts and ignores.
//!
//! Counsel's static checker reads the *source text*: it parses the function,
//! the `#[pre]`/`#[post]`/`#[case]` expressions, and the body, then asks
//! Briev's prover. That works on a file — but a crate still has to compile, and
//! rustc rejects attributes it does not know.
//!
//! These macros exist only to make the syntax legal. Each expands to the item
//! unchanged: no runtime code, no hidden behaviour, nothing to link. The
//! contract itself lives in the attribute's tokens, where the checker reads it.

use proc_macro::TokenStream;

/// `#[pre(expr)]` — a precondition: what must hold to call the function.
///
/// Expands to the annotated item unchanged.
#[proc_macro_attribute]
pub fn pre(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// `#[post(expr)]` — a postcondition over `term`, the returned value.
///
/// Expands to the annotated item unchanged.
#[proc_macro_attribute]
pub fn post(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// `#[case(...)]` — an intended input the contract must hold at.
///
/// Expands to the annotated item unchanged.
#[proc_macro_attribute]
pub fn case(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// `#[invariant(expr)]` — a loop invariant: what holds before, during and after
/// a loop, and what lets a variable-trip-count loop be checked at all.
///
/// Expands to the annotated item unchanged, like the other contract attributes:
/// the tokens are the contract, and `counsel::check` reads them from the source.
#[proc_macro_attribute]
pub fn invariant(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
