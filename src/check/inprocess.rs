// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// inprocess.rs — verify definitions in-process through the `briev-verify` library.
//
// This is the replacement for the compiler subprocess: the same questions are
// asked of the same prover, with no binary to locate, download or version-pin.

use super::build;
use super::prover;
use super::prover::ast::Defn;
use super::{Error, FnVerdict, Outcome, Report, Skip};

/// Whether a report entry names `name` — `"f: reason"` or a bare `"f"`.
fn mentions(entry: &str, name: &str) -> bool {
    entry == name || entry.starts_with(&format!("{name}: "))
}

/// The declaration name of a built item.
fn name_of(defn: &Defn) -> String {
    defn.name.clone()
}

/// Classify one built definition against the prover's report.
///
/// Note the last arm: a definition named nowhere was never examined, so it is
/// reported `Unproven`. Inferring `Proven` from absence is exactly the false
/// proof this crate must never ship.
fn outcome_of(name: &str, report: &prover::Report) -> Outcome {
    if report.violated.iter().any(|entry| mentions(entry, name)) {
        return Outcome::Violated;
    }
    if report.unproven.iter().any(|entry| mentions(entry, name)) {
        return Outcome::Unproven;
    }
    if report.proven.iter().any(|proven| proven == name) {
        // A case proves one concrete input, never the general contract.
        if name.contains("__case") {
            return Outcome::ProvenForCase;
        }
        return Outcome::Proven;
    }
    Outcome::Unproven
}

/// Whether an item is gated behind `#[cfg(test)]`.
///
/// Test bodies are not the subject of a contract census: they are full of
/// `assert!`/`println!` macros, they are not called in production, and including them
/// inflated the macro rejection count by 819 in the first brievc census.
fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|attr| attr.path().is_ident("cfg") && format!("{:?}", attr.meta).contains("test"))
}

/// Every function in a file, including methods inside `impl` blocks and
/// functions inside inline modules, excluding `#[cfg(test)]` declarations.
///
/// `file.items` alone yields only top-level free functions, which understates a
/// real Rust codebase by more than half — most of brievc's logic is methods. A
/// census that silently omits them is worse than no census, because its number
/// looks authoritative. Re-exports (`mod x;`) are not followed: they are a
/// different file, and each file is checked on its own.
fn collect_functions(
    items: &[syn::Item],
    out: &mut Vec<(String, syn::ItemFn)>,
    excluded: &mut usize,
) {
    for item in items {
        match item {
            syn::Item::Fn(item_fn) => {
                if is_cfg_test(&item_fn.attrs) {
                    *excluded += 1;
                    continue;
                }
                out.push((item_fn.sig.ident.to_string(), item_fn.clone()));
            }
            syn::Item::Impl(item_impl) => {
                if is_cfg_test(&item_impl.attrs) {
                    *excluded += 1;
                    continue;
                }
                let owner = type_name(&item_impl.self_ty);
                for entry in &item_impl.items {
                    if let syn::ImplItem::Fn(item_fn) = entry {
                        if is_cfg_test(&item_fn.attrs) {
                            *excluded += 1;
                            continue;
                        }
                        let name = match &owner {
                            Some(owner) => format!("{owner}::{}", item_fn.sig.ident),
                            None => item_fn.sig.ident.to_string(),
                        };
                        // `ImplItemFn` and `ItemFn` are distinct types with no
                        // conversion in syn; the builder consumes the free-function
                        // form, so rebuild it from the same parts (`defaultness`
                        // does not exist on a free function).
                        let free = syn::ItemFn {
                            attrs: item_fn.attrs.clone(),
                            vis: item_fn.vis.clone(),
                            sig: item_fn.sig.clone(),
                            block: Box::new(item_fn.block.clone()),
                        };
                        out.push((name, free));
                    }
                }
            }
            syn::Item::Mod(module) => {
                if is_cfg_test(&module.attrs) {
                    *excluded += 1;
                    continue;
                }
                if let Some((_, inner)) = &module.content {
                    collect_functions(inner, out, excluded);
                }
            }
            _ => {}
        }
    }
}

/// The bare type name of an `impl` block's self type, when it is a plain path.
fn type_name(ty: &syn::Type) -> Option<String> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

/// Verify the contracted functions in a Rust source string, in-process.
///
/// Functions carrying neither `#[pre]` nor `#[post]` are skipped: only written
/// contracts are obligations.
pub fn check_source(source: &str) -> Result<Report, Error> {
    let file = syn::parse_file(source).map_err(|e| Error::Parse(e.to_string()))?;
    // A single unsupported function must not hide the provable ones beside it, so
    // each is built independently and a rejection becomes a warning. A file whose
    // functions all fail still errors rather than reporting an empty pass.
    let mut items = Vec::new();
    let mut skips: Vec<Skip> = Vec::new();
    let mut warnings = Vec::new();
    let mut first_error: Option<Error> = None;
    let mut contracted = 0usize;
    // Calls with no summary are treated as opaque, which is sound but hides the
    // interprocedural gap. Count them so the file reports the assumption.
    let mut opaque_assumed = 0usize;
    // Method calls are opaque too, and they may mutate their receiver, so the
    // count is reported for the same reason.
    let mut methods_assumed = 0usize;
    // Calls are resolved on the `syn` tree first, so the builder only ever sees
    // value expressions. Summaries are computed once for the whole file.
    let sums = build::build_summaries(&file.items);
    let table = build::type_table(&file.items);
    let mut candidates: Vec<(String, syn::ItemFn)> = Vec::new();
    let mut excluded = 0usize;
    collect_functions(&file.items, &mut candidates, &mut excluded);
    for (name, item_fn) in &candidates {
        if !build::has_contract(item_fn) {
            // Enumerate it anyway: an unannotated function is neither verified nor
            // unknown, it was never an obligation. Reporting the skip is what makes
            // a census over a real codebase possible at all.
            let (body, body_code, opaque) = match build::probe_body(item_fn, &sums, &table) {
                Ok(assumptions) => (None, None, assumptions),
                Err(reason) => (
                    Some(reason.to_string()),
                    Some(super::skip_code(&reason.to_string())),
                    0,
                ),
            };
            skips.push(Skip {
                name: name.clone(),
                reason: "no contract".to_string(),
                code: "no contract".to_string(),
                body,
                body_code,
                opaque,
            });
            continue;
        }
        contracted += 1;
        let inlined = match build::inline_calls(item_fn, &sums, &table) {
            Ok(result) => {
                opaque_assumed += result.unresolved;
                methods_assumed += result.methods;
                result.item
            }
            Err(reason) => {
                skips.push(Skip {
                    name: name.clone(),
                    reason: reason.to_string(),
                    code: super::skip_code(&reason.to_string()),
                    body: None,
                    body_code: None,
                    opaque: 0,
                });
                warnings.push(format!("skipped: {reason}"));
                if first_error.is_none() {
                    first_error = Some(reason);
                }
                continue;
            }
        };
        match build::definition_for(&inlined) {
            Ok(found) => items.extend(found),
            Err(reason) => {
                skips.push(Skip {
                    name: name.clone(),
                    reason: reason.to_string(),
                    code: super::skip_code(&reason.to_string()),
                    body: None,
                    body_code: None,
                    opaque: 0,
                });
                warnings.push(format!("skipped: {reason}"));
                if first_error.is_none() {
                    first_error = Some(reason);
                }
            }
        }
    }
    if excluded > 0 {
        warnings.push(format!(
            "{excluded} test declaration(s) excluded from the census"
        ));
    }
    if methods_assumed > 0 {
        warnings.push(format!(
            "{methods_assumed} method call(s) treated as opaque: their result and any\n             mutation of the receiver are unmodelled"
        ));
    }
    if opaque_assumed > 0 {
        warnings.push(format!(
            "{opaque_assumed} call(s) treated as opaque: no contract or body was available"
        ));
    }
    if contracted > 0 && items.is_empty() {
        return Err(first_error.unwrap_or(Error::Unsupported(
            "no contracted function could be verified".to_string(),
        )));
    }
    if items.is_empty() {
        // No contracted function survived, but the skips are the answer to
        // "how much of this tree would you accept?" — returning a default report
        // here would throw the census away and report a clean-looking file.
        return Ok(Report {
            verdicts: Vec::new(),
            skips,
            warnings,
        });
    }
    let report = prover::verify(&items);
    let verdicts = items
        .iter()
        .map(|item| {
            let name = name_of(item);
            // Classification uses the generated name; the report speaks about the
            // function the author wrote (`double__case0` is `double (case 0)`).
            let outcome = outcome_of(&name, &report);
            FnVerdict {
                name: super::display_name(&name),
                outcome,
            }
        })
        .collect();
    Ok(Report {
        verdicts,
        skips,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verdicts for one source snippet, in-process.
    fn verdicts(source: &str) -> Vec<(String, Outcome)> {
        check_source(source)
            .expect("in-process check")
            .verdicts
            .into_iter()
            .map(|v| (v.name, v.outcome))
            .collect()
    }

    /// Methods inside `impl` blocks and functions inside inline modules are
    /// enumerated too. Without this a census of a real Rust codebase undercounts
    /// by more than half, and the number it prints looks authoritative.
    #[test]
    fn enumerates_methods_and_inline_modules() {
        let report = check_source(
            "struct S;\nimpl S { fn method(&self) -> i64 { 1 } }\n\
             mod inner { fn nested() -> i64 { 2 } }\n",
        )
        .expect("the census must not fail on plain items");
        let names: Vec<&str> = report.skips.iter().map(|skip| skip.name.as_str()).collect();
        assert!(
            names.contains(&"S::method"),
            "methods must be enumerated, with their owner: {names:?}"
        );
        assert!(
            names.contains(&"nested"),
            "functions in inline modules must be enumerated: {names:?}"
        );
    }

    /// A boolean literal is an integer in this model, exactly as comparisons fold
    /// to 0/1. Rejecting it blocked 2,802 brievc functions for no semantic reason.
    #[test]
    fn boolean_literals_are_integers() {
        assert_eq!(
            verdicts("#[pre(true)]\n#[post(term == 1)]\nfn one() -> i64 { 1 }\n"),
            vec![("one".to_string(), Outcome::Proven)]
        );
    }

    /// A `bool` return is admitted like a `bool` literal: both are 0/1 here.
    #[test]
    fn a_boolean_return_type_is_admitted() {
        let report = check_source("fn positive(a: i64) -> bool { a > 0 }\n").expect("census");
        assert_eq!(report.skips.len(), 1, "one function");
        assert_eq!(
            report.skips[0].body, None,
            "a bool-returning body must be inside the subset"
        );
    }

    /// L2a: an unmodelled computation becomes an opaque value instead of blocking the
    /// function, so the rest of the body still checks.
    #[test]
    fn a_constructor_does_not_block_translation() {
        assert_eq!(
            verdicts("#[post(term == 0)]\nfn f(x: i64) -> i64 { let e = W::V(x); 0 }\n"),
            vec![("f".to_string(), Outcome::Proven)]
        );
    }

    /// Two occurrences are two distinct unknowns. Sharing one symbol would assume the
    /// callee is pure, and a side-effecting callee could then be "proven"
    /// deterministic - a false proof. `a - b == 0` must stay unproven.
    #[test]
    fn two_constructor_occurrences_are_not_assumed_equal() {
        assert_eq!(
            verdicts(
                "#[post(term == 0)]\nfn f(x: i64) -> i64 { let a = W::V(x); let b = W::V(x); a - b }\n"
            ),
            vec![("f".to_string(), Outcome::Unproven)]
        );
    }

    /// A literal `match` is lowered to if/else, so it needs no new prover machinery.
    #[test]
    fn a_literal_match_is_lowered() {
        assert_eq!(
            verdicts(
                "#[pre(x == 2)]\n#[post(term == 2)]\nfn f(x: i64) -> i64 { match x { 2 => 2, _ => 0 } }\n"
            ),
            vec![("f".to_string(), Outcome::Proven)]
        );
    }

    /// The scrutinee is duplicated per comparison, so a call there must be refused
    /// rather than silently evaluated several times.
    #[test]
    fn a_match_on_a_call_is_refused() {
        let report = check_source(
            "#[post(term >= 0)]\nfn f(x: i64) -> i64 { match g(x) { 1 => 1, _ => 0 } }\n",
        )
        .expect_err("a duplicated scrutinee must be refused");
        assert!(
            report.to_string().contains("subset"),
            "the refusal must name the subset: {report}"
        );
    }

    /// A `match` with no catch-all arm must not be lowered: it would silently change
    /// behaviour for unmatched values.
    #[test]
    fn a_match_without_a_catch_all_is_refused() {
        let err = check_source("#[post(term == 0)]\nfn f(x: i64) -> i64 { match x { 1 => 1 } }\n")
            .expect_err("a non-exhaustive match must be refused");
        assert!(err.to_string().contains("match"), "{err}");
    }

    /// `let x: T = e;` is the same binding with an annotation rustc already checked.
    #[test]
    fn a_type_ascribed_binding_is_admitted() {
        assert_eq!(
            verdicts("#[post(term == 2)]\nfn f() -> i64 { let x: i64 = 2; x }\n"),
            vec![("f".to_string(), Outcome::Proven)]
        );
    }

    /// A destructuring binding still needs a value model for tuples, so it is refused
    /// rather than approximated.
    #[test]
    fn a_destructuring_binding_is_refused() {
        let err = check_source("#[post(term == 1)]\nfn f() -> i64 { let (a, _b) = (1, 2); a }\n")
            .expect_err("tuple destructuring is outside the subset");
        assert!(err.to_string().contains("plain `let"), "{err}");
    }

    /// A `&self` receiver is one more opaque handle: its presence blocked 2,639
    /// functions, which was an admission decision rather than a modelling one.
    #[test]
    fn a_self_receiver_is_admitted_as_opaque() {
        assert_eq!(
            verdicts("#[post(term == x)]\nfn get(&self, x: i64) -> i64 { x }\n"),
            vec![("get".to_string(), Outcome::Proven)]
        );
    }

    /// A non-integer return is opaque too, so the body is still examined. A claim
    /// about `term` then simply fails to prove — it is never proved falsely.
    #[test]
    fn a_non_integer_return_is_admitted_as_opaque() {
        let report = check_source("fn f(x: i64) -> String { W::V(x) }\n").expect("census");
        assert_eq!(report.skips.len(), 1, "one function");
        assert_eq!(
            report.skips[0].body, None,
            "a body ending in an opaque call must translate"
        );
    }

    /// L1: a field write followed by a read resolves to the same flattened symbol.
    #[test]
    fn a_field_write_then_read_resolves() {
        assert_eq!(
            verdicts("#[post(term == 5)]\nfn f(mut s: S) -> i64 { s.x = 5; s.x }\n"),
            vec![("f".to_string(), Outcome::Proven)]
        );
    }

    /// L1: a precondition about a field is a fact about its flattened symbol, so a
    /// contract can drive a proof through a field read.
    #[test]
    fn a_contract_over_a_field_discharges() {
        assert_eq!(
            verdicts("#[pre(s.x >= 0)]\n#[post(term >= 0)]\nfn g(s: S) -> i64 { s.x + 1 }\n"),
            vec![("g".to_string(), Outcome::Proven)]
        );
    }

    /// D1: a `match` over a locally constructed enum variant lands by tag comparison.
    #[test]
    fn a_match_over_a_constructed_variant_discharges() {
        assert_eq!(
            verdicts(
                "enum E { A, B }\n#[post(term == 1)]\nfn f() -> i64 { match E::A { E::A => 1, _ => 0 } }\n"
            ),
            vec![("f".to_string(), Outcome::Proven)]
        );
    }

    /// D1: a tuple variant carries a tag too; the payload is dropped, the arm still
    /// lands.
    #[test]
    fn a_match_over_a_payload_variant_discharges() {
        assert_eq!(
            verdicts(
                "enum E { V(i64) }\n#[post(term == 1)]\nfn f() -> i64 { match E::V(5) { E::V(_) => 1, _ => 0 } }\n"
            ),
            vec![("f".to_string(), Outcome::Proven)]
        );
    }

    /// D1: an enum that is not declared in this file stays opaque, so matching it is
    /// refused rather than silently assumed to be any variant. The sound direction is
    /// no verdict at all.
    #[test]
    fn a_match_over_a_foreign_enum_is_refused() {
        check_source("#[post(term == 1)]\nfn f() -> i64 { match W::A { W::A => 1, _ => 0 } }\n")
            .expect_err("a foreign-enum match has no known tags");
    }

    /// D3: the prelude catalog makes `Some`/`None` constructors, so a locally built
    /// `Option` matches by the same tag rule as a same-file enum.
    #[test]
    fn a_match_over_option_constructors_discharges() {
        assert_eq!(
            verdicts(
                "#[post(term == 1)]\nfn f() -> i64 { match Some(5) { Some(_) => 1, _ => 0 } }\n"
            ),
            vec![("f".to_string(), Outcome::Proven)]
        );
        assert_eq!(
            verdicts("#[post(term == 0)]\nfn g() -> i64 { match None { Some(_) => 1, _ => 0 } }\n"),
            vec![("g".to_string(), Outcome::Proven)]
        );
    }

    /// A true identity is discharged with no compiler, no text and no process.
    #[test]
    fn proves_a_true_identity() {
        assert_eq!(
            verdicts(
                "#[pre(a >= 0)]\n#[post(term == a + a)]\nfn double(a: i64) -> i64 { a + a }\n"
            ),
            vec![("double".to_string(), Outcome::Proven)]
        );
    }

    /// A false postcondition is refuted, not silently accepted.
    ///
    /// The body is constant on purpose: a parameter-dependent contradiction
    /// (`a + a == 0` for `a >= 0`) is genuinely refutable only at a = 1, and a
    /// solver model with a free parameter stays advisory by design, so the honest
    /// verdict there is Unproven rather than Violated.
    #[test]
    fn refutes_a_false_contract() {
        assert_eq!(
            verdicts("#[post(term == 0)]\nfn lie(a: i64) -> i64 { 5 }\n"),
            vec![("lie".to_string(), Outcome::Violated)]
        );
    }

    /// A parameter-dependent contradiction is advisory, not a refutation.
    #[test]
    fn leaves_a_parameter_dependent_contradiction_advisory() {
        assert_eq!(
            verdicts("#[pre(a >= 0)]\n#[post(term == 0)]\nfn maybe(a: i64) -> i64 { a + a }\n"),
            vec![("maybe".to_string(), Outcome::Unproven)]
        );
    }

    /// A buffer-shaped parameter is admitted as opaque: the contract is about the
    /// integers, and any actual use of the buffer in the body is still refused.
    #[test]
    fn a_buffer_parameter_is_admitted_as_opaque() {
        let source = "#[pre(n >= 0)]\n#[post(term >= 0)]\n\
                      fn passes(buffer: &[u8], n: i64) -> i64 { n }\n";
        assert_eq!(
            verdicts(source),
            vec![("passes".to_string(), Outcome::Proven)]
        );
    }

    /// Admitting the parameter must not admit its *contents*: an indexed use is
    /// still outside the subset and is refused, not assumed.
    #[test]
    fn an_indexed_buffer_use_is_still_refused() {
        let source = "#[post(term >= 0)]\n\
                      fn reads(buffer: &[u8]) -> i64 { buffer[0] as i64 }\n";
        assert!(check_source(source).is_err(), "indexing must stay refused");
    }

    /// Domain facts still pin a narrow width: `u8` cannot exceed 255, so a body
    /// that only holds because of the range is provable.
    #[test]
    fn carries_domain_facts_for_narrow_widths() {
        assert_eq!(
            verdicts("#[post(term <= 255)]\nfn fits(x: u8) -> u8 { x }\n"),
            vec![("fits".to_string(), Outcome::Proven)]
        );
    }

    /// Branches: the unguarded-else path must reach the right goal.
    #[test]
    fn proves_across_both_arms() {
        let source = "#[pre(a >= 0)]\n#[post(term >= 0)]\nfn abs_like(a: i64) -> i64 { if a < 0 { 0 - a } else { a } }\n";
        assert_eq!(
            verdicts(source),
            vec![("abs_like".to_string(), Outcome::Proven)]
        );
    }

    /// A supported method call desugars to the same guarded branches as before.
    #[test]
    fn proves_a_method_call_tail() {
        assert_eq!(
            verdicts("#[post(term >= 1)]\nfn at_least_one(x: i64) -> i64 { x.max(1) }\n"),
            vec![("at_least_one".to_string(), Outcome::Proven)]
        );
    }

    /// A constant-bounded loop is unrolled and proved.
    #[test]
    fn proves_a_constant_bounded_loop() {
        let source = "#[post(term == 3)]\nfn sum_three() -> i64 { let mut acc = 0; for i in 0..3 { acc = acc + i; } acc }\n";
        assert_eq!(
            verdicts(source),
            vec![("sum_three".to_string(), Outcome::Proven)]
        );
    }

    /// A dynamic bound is refused rather than unrolled unsoundly.
    #[test]
    fn refuses_a_dynamic_loop_bound() {
        let err = check_source(
            "#[post(term >= 0)]\nfn f(n: i64) -> i64 { let mut a = 0; for i in 0..n { a = a + 1; } a }\n",
        )
        .expect_err("dynamic bounds must be refused");
        assert!(
            format!("{err}").contains("bounds must be integer literals"),
            "{err}"
        );
    }

    /// An invariant-carrying loop yields the three Hoare obligations, all proved.
    #[test]
    fn proves_all_three_loop_obligations() {
        let source = "#[pre(n >= 0 && n <= 1000)]\n#[post(term >= 0)]\n#[invariant(acc == i && i <= n)]\nfn count(n: i64) -> i64 { let mut acc = 0; for i in 0..n { acc = acc + 1; } acc }\n";
        let seen = verdicts(source);
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert!(seen.iter().all(|(_, o)| *o == Outcome::Proven), "{seen:?}");
    }

    /// A wrong invariant is caught at the obligation it actually breaks.
    #[test]
    fn a_wrong_invariant_fails_at_entry() {
        let source = "#[pre(n >= 0 && n <= 1000)]\n#[post(term >= 0)]\n#[invariant(acc == i + 1 && i <= n)]\nfn count_bad(n: i64) -> i64 { let mut acc = 0; for i in 0..n { acc = acc + 1; } acc }\n";
        let seen = verdicts(source);
        let init = seen
            .iter()
            .find(|(n, _)| n == "count_bad_init")
            .expect("init");
        assert_eq!(init.1, Outcome::Violated, "{seen:?}");
    }

    /// A loop whose tail is a method call uses the same desugaring.
    #[test]
    fn proves_a_loop_ending_in_a_method_call() {
        let source = "#[post(term >= 1)]\n#[invariant(best >= 1)]\nfn step(len: usize) -> usize { let mut best = 1; let half = len / 2; for d in 1..=half { if len % d == 0 { best = d; } } best.max(1) }\n";
        let seen = verdicts(source);
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert!(seen.iter().all(|(_, o)| *o == Outcome::Proven), "{seen:?}");
    }

    /// A call to a precondition-free helper resolves to the helper's value.
    ///
    /// The lab kernel `caller.rs` caught this missing after the text emitter was
    /// deleted: inlining lived there, and no test here exercised a call.
    #[test]
    fn resolves_a_call_to_a_pure_helper() {
        let source = "fn inc(a: i64) -> i64 { a + 1 }\n\
                      #[post(term == a + 2)]\n\
                      fn uses_inc(a: i64) -> i64 { inc(a) + 1 }\n";
        let seen = verdicts(source);
        assert_eq!(seen, vec![("uses_inc".to_string(), Outcome::Proven)]);
    }

    /// A callee with a precondition earns no summary, so its call is refused
    /// rather than assumed — the R3 rule that keeps the proof sound.
    #[test]
    fn refuses_a_call_to_a_preconditioned_helper() {
        let source = "#[pre(a > 0)]\nfn guarded(a: i64) -> i64 { a }\n\
                      #[post(term == 1)]\nfn uses(a: i64) -> i64 { guarded(a) }\n";
        // The helper's precondition is not established at the call site, so inlining
        // it would be unsound. It stays opaque: the caller can never be proven from a
        // claim about the result, and the assumption is reported rather than hidden.
        let report = check_source(source).expect("the other function still verifies");
        assert!(
            report
                .verdicts
                .iter()
                .all(|verdict| verdict.name != "uses" || verdict.outcome != Outcome::Proven),
            "an opaque call result must never be reported as proven: {:?}",
            report.verdicts
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("opaque")),
            "the opaque assumption must be reported: {:?}",
            report.warnings
        );
    }

    /// A declaration the prover never examines is Unproven, never Proven.
    #[test]
    fn an_unexamined_declaration_is_not_proven() {
        // A loop is not built in-process yet, so this must be an error rather
        // than a silent pass.
        let err = check_source("#[post(term >= 0)]\nfn f(n: i64) -> i64 { let mut a = 0; for i in 0..n { a = a + 1; } a }\n");
        assert!(
            err.is_err(),
            "an unbuilt construct must be refused, not proved"
        );
    }
}
