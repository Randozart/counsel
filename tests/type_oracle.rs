// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// type_oracle.rs — the shape-oracle kernel (plan SS18 D4).
//
// Counsel infers types from the file's own declarations (the lightweight rule). The
// heavy tool that audits it is the compiler itself: the fixture modules are included
// below as real `mod`s, so `cargo test` makes rustc type-check them. If any fixture
// ever stops being valid Rust, this crate stops compiling. These tests then assert
// that Counsel's resolver produces exactly the facts rustc has already accepted.
//
// Two oracles, independent: rustc for the SHAPE layer (what is an enum, a variant, a
// field), and Counsel's own contract prover for the ARITHMETIC layer. Neither answers
// for the other.

use counsel::check::build::{type_table, TypeTable};

// Included (not `include_str!` only) so rustc validates the fixtures.
#[path = "oracle_fixtures/catalog.rs"]
mod catalog;

/// Counsel's resolver for fixture source.
fn table_of(source: &str) -> TypeTable {
    let file = syn::parse_file(source).expect("fixture parses as Rust");
    type_table(&file.items)
}

/// Variant tags follow declaration order, which is the contract between a construction
/// and its `match`.
#[test]
fn resolver_tags_match_declaration_order() {
    let table = table_of(include_str!("oracle_fixtures/catalog.rs"));
    assert_eq!(table.variant_tag("Accel", "Try"), Some(0));
    assert_eq!(table.variant_tag("Accel", "Force"), Some(1));
    assert_eq!(table.variant_tag("Accel", "TryAll"), Some(2));
    assert_eq!(table.variant_tag("Accel", "TryAllForce"), Some(3));
    // A variant that is not declared is genuinely unknown, not falsely tagged.
    assert_eq!(table.variant_tag("Accel", "Missing"), None);
}

/// Every enum is indexed; tuple and unit variants are treated alike for tagging.
#[test]
fn resolver_records_each_enum_independently() {
    let table = table_of(include_str!("oracle_fixtures/catalog.rs"));
    assert_eq!(table.variant_tag("Opt", "Some"), Some(0));
    assert_eq!(table.variant_tag("Opt", "None"), Some(1));
    // Distinct enums reuse tag space separately; there is no cross-enum comparison.
    assert_eq!(table.variant_tag("Accel", "Try"), Some(0));
}

/// A struct is not an enum: `Rect::x` looks like `Enum::Variant` but must not tag.
#[test]
fn a_struct_name_is_not_a_variant() {
    let table = table_of(include_str!("oracle_fixtures/catalog.rs"));
    assert_eq!(table.variant_tag("Rect", "x"), None);
    assert_eq!(table.variant_tag("Rect", "y"), None);
}
