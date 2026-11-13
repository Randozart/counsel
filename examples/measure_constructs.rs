// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Count the constructs that still block real Rust kernels.
//!
//! Grep cannot answer this: `match` on an integer literal and `match` on a
//! struct look identical as text. This parses the tree with `syn` and counts
//! only what appears inside integer-returning functions.
//!
//! Usage: cargo run --features static --example measure_constructs -- <dir>

use std::path::Path;
use syn::visit::{self, Visit};

/// Totals across the scanned tree.
#[derive(Default)]
struct Counts {
    integer_fns: usize,
    matches: usize,
    integer_label_matches: usize,
    whiles: usize,
    casts: usize,
    method_calls: usize,
    /// Method-call names, so the next candidate is chosen from data.
    method_names: std::collections::BTreeMap<String, usize>,
}

/// Whether a type names a primitive integer.
fn is_integer_type(ty: &syn::Type) -> bool {
    const NAMES: [&str; 12] = [
        "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
    ];
    let syn::Type::Path(path) = ty else {
        return false;
    };
    path.path
        .get_ident()
        .is_some_and(|ident| NAMES.contains(&ident.to_string().as_str()))
}

/// Visitor counting the constructs inside one function body.
struct FunctionScan<'a> {
    counts: &'a mut Counts,
}

impl<'ast> Visit<'ast> for FunctionScan<'_> {
    /// Count `match` expressions, and separately those desugarable into `when`s.
    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        self.counts.matches += 1;
        if node.arms.iter().all(|arm| arm_is_integer_label(&arm.pat)) {
            self.counts.integer_label_matches += 1;
        }
        visit::visit_expr_match(self, node);
    }

    /// Count `while` loops: the invariant machinery is `for`-shaped.
    fn visit_expr_while(&mut self, node: &'ast syn::ExprWhile) {
        self.counts.whiles += 1;
        visit::visit_expr_while(self, node);
    }

    /// Count `as` casts, which are width-dependent and so are not free.
    fn visit_expr_cast(&mut self, node: &'ast syn::ExprCast) {
        self.counts.casts += 1;
        visit::visit_expr_cast(self, node);
    }

    /// Count method calls of any kind.
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        self.counts.method_calls += 1;
        *self
            .counts
            .method_names
            .entry(node.method.to_string())
            .or_insert(0) += 1;
        visit::visit_expr_method_call(self, node);
    }
}

/// Whether an arm pattern is an integer literal, literal range, wildcard, or an
/// or-pattern of those — the shapes a `when` chain could express.
fn arm_is_integer_label(pat: &syn::Pat) -> bool {
    match pat {
        syn::Pat::Wild(_) => true,
        syn::Pat::Lit(lit) => matches!(lit.lit, syn::Lit::Int(_)),
        syn::Pat::Range(_) => true,
        syn::Pat::Or(or) => or.cases.iter().all(arm_is_integer_label),
        syn::Pat::Paren(paren) => arm_is_integer_label(&paren.pat),
        _ => false,
    }
}

/// Scan one file, counting only functions that return a primitive integer.
fn scan_file(path: &Path, counts: &mut Counts) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(file) = syn::parse_file(&text) else {
        return;
    };
    for item in &file.items {
        let syn::Item::Fn(item_fn) = item else {
            continue;
        };
        let syn::ReturnType::Type(_, ty) = &item_fn.sig.output else {
            continue;
        };
        if !is_integer_type(ty) {
            continue;
        }
        counts.integer_fns += 1;
        let mut scan = FunctionScan {
            counts: &mut *counts,
        };
        scan.visit_block(&item_fn.block);
    }
}

/// Walk a directory recursively, scanning every `.rs` file.
fn scan_dir(dir: &Path, counts: &mut Counts) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir(&path, counts);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            scan_file(&path, counts);
        }
    }
}

fn main() {
    let Some(root) = std::env::args().nth(1) else {
        eprintln!("usage: measure_constructs <dir>");
        std::process::exit(2);
    };
    let mut counts = Counts::default();
    scan_dir(Path::new(&root), &mut counts);
    println!("integer-returning functions: {}", counts.integer_fns);
    println!("  match expressions:          {}", counts.matches);
    println!("    integer-label only:       {}", counts.integer_label_matches);
    println!("  while loops:                {}", counts.whiles);
    println!("  as casts:                   {}", counts.casts);
    println!("  method calls:               {}", counts.method_calls);
    let mut ranked: Vec<(&String, &usize)> = counts.method_names.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (name, count) in ranked.into_iter().take(12) {
        println!("    .{name:<24} {count}");
    }
}
