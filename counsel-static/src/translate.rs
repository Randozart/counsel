// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// src/translate.rs — restricted pure-integer Rust → Briev `defn` source.
//
// The subset is deliberately tiny so the translation is obviously faithful:
// `i64` parameters and return, `let` bindings, integer arithmetic, comparison
// and boolean operators, `if`/`else` branches (translated to Briev `when`
// guards), `return`, and a trailing expression. Anything else is rejected as
// unsupported rather than approximated — a wrong translation would be a
// wrong proof.
//
// Contracts arrive as ordinary attributes:
//
//   #[pre(a >= 0)]
//   #[post(term >= 0)]
//   fn double(a: i64) -> i64 { a * 2 }
//
// and become `defn double(a: Int) [a >= 0][term >= 0] -> Int { term a * 2; };`

use crate::Error;

/// One function translated into Briev source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslatedFn {
    /// The Rust function name (unchanged in Briev).
    pub name: String,
    /// A complete, compilable Briev `defn` declaration.
    pub briev: String,
}

/// Translate every contracted function in a parsed Rust file.
///
/// Returns the translated declarations plus advisory warnings for renamed or
/// misspelt contract attributes. Functions carrying neither `#[pre]` nor
/// `#[post]` are skipped — only declared contracts are propositions to prove.
pub fn translate_file(file: &syn::File) -> Result<(Vec<TranslatedFn>, Vec<String>), Error> {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for item in &file.items {
        let syn::Item::Fn(item_fn) = item else {
            continue;
        };
        let pre = contract_attr(&item_fn.attrs, "pre")?;
        let post = contract_attr(&item_fn.attrs, "post")?;
        warnings.extend(attribute_warnings(&item_fn.attrs));
        if pre.is_none() && post.is_none() {
            continue;
        }
        out.push(translate_fn(item_fn, pre, post)?);
    }
    Ok((out, warnings))
}

/// Translate one contracted function.
fn translate_fn(
    item_fn: &syn::ItemFn,
    pre: Option<String>,
    post: Option<String>,
) -> Result<TranslatedFn, Error> {
    let name = item_fn.sig.ident.to_string();
    let params = translate_params(&name, &item_fn.sig)?;
    let pre = pre.unwrap_or_else(|| "true".to_string());
    let post = post.unwrap_or_else(|| "true".to_string());
    let body = translate_block(&item_fn.block.stmts)?;
    if body.trim().is_empty() {
        return Err(Error::Unsupported(format!(
            "`{name}`: empty body has no value to verify"
        )));
    }
    let briev = format!("defn {name}({params}) [{pre}][{post}] -> Int {{\n    {body}\n}};\n");
    Ok(TranslatedFn { name, briev })
}

/// Translate the parameter list, requiring plain `i64` parameters.
fn translate_params(name: &str, sig: &syn::Signature) -> Result<String, Error> {
    let mut params = Vec::new();
    for input in &sig.inputs {
        let syn::FnArg::Typed(pat_ty) = input else {
            return Err(Error::Unsupported(format!(
                "`{name}`: methods with `self` are not supported"
            )));
        };
        require_i64(name, "parameter", &pat_ty.ty)?;
        let syn::Pat::Ident(pat_ident) = pat_ty.pat.as_ref() else {
            return Err(Error::Unsupported(format!(
                "`{name}`: only plain identifier parameters are supported"
            )));
        };
        params.push(format!("{}: Int", pat_ident.ident));
    }
    if let syn::ReturnType::Type(_, ty) = &sig.output {
        require_i64(name, "return type", ty)?;
    }
    Ok(params.join(", "))
}

/// Reject any type other than `i64` at the boundary.
fn require_i64(name: &str, what: &str, ty: &syn::Type) -> Result<(), Error> {
    let syn::Type::Path(path) = ty else {
        return Err(Error::Unsupported(format!(
            "`{name}`: {what} must be `i64`"
        )));
    };
    let last = path.path.segments.last();
    if last.is_some_and(|s| s.ident == "i64") {
        Ok(())
    } else {
        Err(Error::Unsupported(format!(
            "`{name}`: {what} must be `i64` (the pure-integer subset)"
        )))
    }
}

/// Read an attribute's single expression argument, e.g. `#[pre(a >= 0)]`.
/// Read a contract attribute, rejecting duplicates.
///
/// `contract_attr` must scan every attribute: returning on the first match
/// silently dropped the second `#[pre(...)]`, so a repeated slot is an error
/// rather than an invisible precedence rule.
fn contract_attr(attrs: &[syn::Attribute], name: &str) -> Result<Option<String>, Error> {
    let mut found: Option<String> = None;
    for attr in attrs {
        if !attr.path().is_ident(name) {
            continue;
        }
        if found.is_some() {
            return Err(Error::Contract(format!(
                "more than one `#[{name}(...)]` on this function — keep one"
            )));
        }
        let expr: syn::Expr = attr
            .parse_args()
            .map_err(|e| Error::Parse(format!("`#[{name}(...)]`: {e}")))?;
        found = Some(translate_expr(&expr, true)?);
    }
    Ok(found)
}

/// Advisory warnings for contract attributes that were renamed or misspelt.
///
/// A misspelt attribute used to mean "no contract, no verification, no
/// warning" — the function was skipped silently. Ordinary Rust attributes
/// (`#[test]`, `#[inline]`, `#[derive(...)]`) never warn: renamed names are
/// matched exactly, and a near-miss is only considered when the attribute
/// carries arguments, which contract attributes always do.
fn attribute_warnings(attrs: &[syn::Attribute]) -> Vec<String> {
    let mut out = Vec::new();
    for attr in attrs {
        let Some(ident) = attr.path().get_ident() else {
            continue;
        };
        let name = ident.to_string();
        if matches!(name.as_str(), "pre" | "post" | "case") {
            continue;
        }
        let hint = match removed_name_hint(&name) {
            Some(hint) => Some(hint),
            None if !matches!(attr.meta, syn::Meta::Path(_)) => near_miss_hint(&name),
            None => None,
        };
        if let Some(hint) = hint {
            out.push(format!(
                "`#[{name}]` is not a Counsel contract attribute — {hint}; \
                 accepted: `#[pre(...)]`, `#[post(...)]`, `#[case(...)]`"
            ));
        }
    }
    out
}

/// The replacement for an attribute name this crate used to accept.
fn removed_name_hint(name: &str) -> Option<&'static str> {
    match name {
        "requires" => Some("it was renamed to `#[pre(...)]`"),
        "ensures" => Some("it was renamed to `#[post(...)]`"),
        "precondition" | "pre_condition" => Some("use `#[pre(...)]`"),
        "postcondition" | "post_condition" => Some("use `#[post(...)]`"),
        "intent" | "intend" | "expect" | "expected" | "required" => {
            Some("use `#[case(...)]` to list intended cases")
        }
        _ => None,
    }
}

/// A "did you mean" hint for a name one edit away from a contract attribute.
fn near_miss_hint(name: &str) -> Option<&'static str> {
    ["pre", "post", "case"]
        .into_iter()
        .find(|candidate| edit_distance(name, candidate) <= 1)
        .map(|candidate| match candidate {
            "pre" => "did you mean `#[pre(...)]`?",
            "post" => "did you mean `#[post(...)]`?",
            _ => "did you mean `#[case(...)]`?",
        })
}

/// Levenshtein distance; names are a few bytes, so the quadratic cost is moot.
fn edit_distance(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b_chars.len()).collect();
    for (i, a_char) in a.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, b_char) in b_chars.iter().enumerate() {
            let substitution = previous[j] + usize::from(a_char != *b_char);
            let insertion = current[j] + 1;
            let deletion = previous[j + 1] + 1;
            current.push(substitution.min(insertion).min(deletion));
        }
        previous = current;
    }
    previous[b_chars.len()]
}

/// Translate a statement block into Briev statements.
fn translate_block(stmts: &[syn::Stmt]) -> Result<String, Error> {
    let mut out = Vec::new();
    let last_index = stmts.len().saturating_sub(1);
    for (index, stmt) in stmts.iter().enumerate() {
        translate_stmt(stmt, index == last_index, &mut out)?;
    }
    Ok(out.join("\n    "))
}

/// Translate one statement, appending zero or more Briev statements.
fn translate_stmt(stmt: &syn::Stmt, is_last: bool, out: &mut Vec<String>) -> Result<(), Error> {
    match stmt {
        syn::Stmt::Local(local) => translate_local(local, out),
        syn::Stmt::Expr(expr, semi) => translate_expr_stmt(expr, semi.is_none(), is_last, out),
        other => Err(Error::Unsupported(format!(
            "unsupported statement: {other:?}"
        ))),
    }
}

/// Translate `let x: i64 = expr;` into `let x: Int = expr;`.
fn translate_local(local: &syn::Local, out: &mut Vec<String>) -> Result<(), Error> {
    let syn::Pat::Ident(pat_ident) = &local.pat else {
        return Err(Error::Unsupported(
            "only plain `let x = ...;` bindings are supported".to_string(),
        ));
    };
    let Some(init) = &local.init else {
        return Err(Error::Unsupported(
            "`let` without an initializer is not supported".to_string(),
        ));
    };
    let value = translate_expr(&init.expr, true)?;
    out.push(format!("let {}: Int = {};", pat_ident.ident, value));
    Ok(())
}

/// Translate an expression statement: `return`, `if`, or a final tail value.
fn translate_expr_stmt(
    expr: &syn::Expr,
    no_semi: bool,
    is_last: bool,
    out: &mut Vec<String>,
) -> Result<(), Error> {
    match expr {
        syn::Expr::Return(ret) => translate_return(ret, out),
        syn::Expr::If(node) => {
            out.push(translate_if(node)?);
            Ok(())
        }
        // A trailing expression is the function's value.
        other if no_semi && is_last => {
            out.push(format!("term {};", translate_expr(other, true)?));
            Ok(())
        }
        other => Err(Error::Unsupported(format!(
            "unsupported expression statement: {other:?}"
        ))),
    }
}

/// Translate `return e;` into `term e;`.
fn translate_return(ret: &syn::ExprReturn, out: &mut Vec<String>) -> Result<(), Error> {
    let Some(value) = &ret.expr else {
        return Err(Error::Unsupported(
            "bare `return;` has no value in the pure-integer subset".to_string(),
        ));
    };
    out.push(format!("term {};", translate_expr(value, true)?));
    Ok(())
}

/// Translate `if cond { A } else { B }` into Briev `when` guards.
///
/// When both arms always produce a value, the else body is emitted *unguarded*
/// after the then-guard: reaching it implies the condition failed, because the
/// then-arm always terminated. That is both simpler and avoids a Briev backend
/// bug where two consecutive fully-terminating `when` guards emit a dangling
/// `ret i64 %undefined` (reproduced 2026-11-11; see BUGS.md).
///
/// When the arms may fall through, the else arm is guarded by the *inverted*
/// condition, which also feeds a real numeric fact to the bounds prover
/// (`a <= 5` rather than `!(a > 5)`).
fn translate_if(node: &syn::ExprIf) -> Result<String, Error> {
    let cond = translate_expr(&node.cond, true)?;
    let then_body = translate_block(&node.then_branch.stmts)?;
    let mut text = format!("when {cond} {{ {then_body} }};");
    let Some((_, else_expr)) = &node.else_branch else {
        return Ok(text);
    };
    let else_body = translate_branch(else_expr)?;
    if block_terminates(&node.then_branch.stmts) && expr_terminates(else_expr) {
        text.push_str(&format!("\n    {else_body}"));
    } else {
        let inverted = invert_condition(&node.cond)?;
        text.push_str(&format!("\n    when {inverted} {{ {else_body} }};"));
    }
    Ok(text)
}

/// Whether a statement block always produces a value (and so always returns).
fn block_terminates(stmts: &[syn::Stmt]) -> bool {
    let Some(last) = stmts.last() else {
        return false;
    };
    match last {
        syn::Stmt::Expr(syn::Expr::Return(_), _) => true,
        syn::Stmt::Expr(expr, None) => expr_terminates(expr),
        syn::Stmt::Expr(expr, Some(_)) => matches!(expr, syn::Expr::Return(_)),
        _ => false,
    }
}

/// Whether an expression used as a branch always produces a value.
fn expr_terminates(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::If(node) => if_terminates(node),
        syn::Expr::Block(block) => block_terminates(&block.block.stmts),
        // Any other trailing expression is the branch's value.
        _ => true,
    }
}

/// Whether an `if`/`else` always produces a value (both arms must).
fn if_terminates(node: &syn::ExprIf) -> bool {
    let then_ok = block_terminates(&node.then_branch.stmts);
    let else_ok = match &node.else_branch {
        Some((_, else_expr)) => expr_terminates(else_expr),
        None => false,
    };
    then_ok && else_ok
}

/// Translate an `else` arm: a block, an `else if` chain, or a bare expression.
fn translate_branch(expr: &syn::Expr) -> Result<String, Error> {
    match expr {
        syn::Expr::Block(block) => translate_block(&block.block.stmts),
        syn::Expr::If(node) => translate_if(node),
        other => Ok(format!("term {};", translate_expr(other, true)?)),
    }
}

/// Invert a condition, preferring an inverted comparison operator.
fn invert_condition(cond: &syn::Expr) -> Result<String, Error> {
    if let syn::Expr::Binary(binary) = strip_parens(cond) {
        if let Some(op) = inverted_op(&binary.op) {
            let left = translate_expr(&binary.left, true)?;
            let right = translate_expr(&binary.right, true)?;
            return Ok(format!("{left} {op} {right}"));
        }
    }
    Ok(format!("!({})", translate_expr(cond, true)?))
}

/// The mirrored comparison operator, when one exists.
fn inverted_op(op: &syn::BinOp) -> Option<&'static str> {
    match op {
        syn::BinOp::Lt(_) => Some(">="),
        syn::BinOp::Le(_) => Some(">"),
        syn::BinOp::Gt(_) => Some("<="),
        syn::BinOp::Ge(_) => Some("<"),
        syn::BinOp::Eq(_) => Some("!="),
        syn::BinOp::Ne(_) => Some("=="),
        _ => None,
    }
}

/// Strip redundant parentheses around an expression.
fn strip_parens(expr: &syn::Expr) -> &syn::Expr {
    match expr {
        syn::Expr::Paren(paren) => strip_parens(&paren.expr),
        other => other,
    }
}

/// Translate an expression to Briev expression syntax.
///
/// `allow_ident` permits bare identifiers (contracts and bodies both need
/// them; kept as a parameter so the intent is explicit at call sites).
fn translate_expr(expr: &syn::Expr, allow_ident: bool) -> Result<String, Error> {
    match expr {
        syn::Expr::Lit(lit) => translate_lit(lit),
        syn::Expr::Path(path) => translate_path(path, allow_ident),
        syn::Expr::Binary(binary) => translate_binary(binary, allow_ident),
        syn::Expr::Unary(unary) => translate_unary(unary, allow_ident),
        syn::Expr::Paren(paren) => Ok(format!("({})", translate_expr(&paren.expr, allow_ident)?)),
        other => Err(Error::Unsupported(format!(
            "unsupported expression (pure-integer subset): {other:?}"
        ))),
    }
}

/// Translate an integer literal.
fn translate_lit(lit: &syn::ExprLit) -> Result<String, Error> {
    match &lit.lit {
        syn::Lit::Int(value) => Ok(value.base10_digits().to_string()),
        other => Err(Error::Unsupported(format!(
            "unsupported literal (integers only): {other:?}"
        ))),
    }
}

/// Translate a path used as a value: a bare identifier or `term`.
fn translate_path(path: &syn::ExprPath, allow_ident: bool) -> Result<String, Error> {
    if !allow_ident || path.qself.is_some() || path.path.segments.len() != 1 {
        return Err(Error::Unsupported(
            "unsupported path expression (bare identifiers only)".to_string(),
        ));
    }
    let Some(segment) = path.path.segments.first() else {
        return Err(Error::Unsupported("empty path expression".to_string()));
    };
    if !matches!(segment.arguments, syn::PathArguments::None) {
        return Err(Error::Unsupported(
            "generic arguments are not supported".to_string(),
        ));
    }
    Ok(segment.ident.to_string())
}

/// Translate a binary operation.
fn translate_binary(binary: &syn::ExprBinary, allow_ident: bool) -> Result<String, Error> {
    let op = binary_op(&binary.op)?;
    let left = translate_expr(&binary.left, allow_ident)?;
    let right = translate_expr(&binary.right, allow_ident)?;
    Ok(format!("{left} {op} {right}"))
}

/// Map a Rust binary operator onto its Briev spelling.
fn binary_op(op: &syn::BinOp) -> Result<&'static str, Error> {
    match op {
        syn::BinOp::Add(_) => Ok("+"),
        syn::BinOp::Sub(_) => Ok("-"),
        syn::BinOp::Mul(_) => Ok("*"),
        syn::BinOp::Div(_) => Ok("/"),
        syn::BinOp::Rem(_) => Ok("%"),
        syn::BinOp::Eq(_) => Ok("=="),
        syn::BinOp::Ne(_) => Ok("!="),
        syn::BinOp::Lt(_) => Ok("<"),
        syn::BinOp::Le(_) => Ok("<="),
        syn::BinOp::Gt(_) => Ok(">"),
        syn::BinOp::Ge(_) => Ok(">="),
        syn::BinOp::And(_) => Ok("&&"),
        syn::BinOp::Or(_) => Ok("||"),
        other => Err(Error::Unsupported(format!(
            "unsupported binary operator: {other:?}"
        ))),
    }
}

/// Translate a unary negation or boolean not.
fn translate_unary(unary: &syn::ExprUnary, allow_ident: bool) -> Result<String, Error> {
    let inner = translate_expr(&unary.expr, allow_ident)?;
    match unary.op {
        syn::UnOp::Neg(_) => Ok(format!("-{inner}")),
        syn::UnOp::Not(_) => Ok(format!("!({inner})")),
        _ => Err(Error::Unsupported("unsupported unary operator".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse Rust source and translate its contracted functions.
    fn translate(source: &str) -> Vec<TranslatedFn> {
        let file = syn::parse_file(source).expect("parse rust");
        translate_file(&file).expect("translate").0
    }

    #[test]
    fn tail_expression_becomes_term() {
        let out =
            translate("#[pre(a >= 0)]\n#[post(term >= a)]\nfn double(a: i64) -> i64 { a * 2 }\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "double");
        assert!(out[0].briev.contains("[a >= 0][term >= a]"));
        assert!(out[0].briev.contains("term a * 2;"));
    }

    #[test]
    fn return_and_if_become_guard_and_term() {
        let out =
            translate("#[post(term >= 0)]\nfn f(a: i64) -> i64 { if a > 5 { return a; } 0 }\n");
        let briev = &out[0].briev;
        assert!(briev.contains("when a > 5 { term a; };"), "{briev}");
        assert!(briev.contains("term 0;"), "{briev}");
    }

    #[test]
    fn else_arm_of_terminating_branches_is_emitted_unguarded() {
        let out =
            translate("#[post(term >= 0)]\nfn f(a: i64) -> i64 { if a > 5 { a } else { 0 } }\n");
        let briev = &out[0].briev;
        assert!(briev.contains("when a > 5 { term a; };"), "{briev}");
        // Both arms produce a value: reaching the else body implies the guard
        // failed, so it is emitted unguarded (avoids the double-terminating
        // guard backend bug — see BUGS.md).
        assert!(briev.contains("term 0;"), "{briev}");
        assert!(!briev.contains("when a <= 5"), "{briev}");
    }

    #[test]
    fn else_arm_of_fall_through_branches_stays_guarded() {
        // The then-arm does not terminate (its last statement is a `let`),
        // so the else arm must stay behind the inverted guard.
        let out = translate(
            "#[post(term >= 0)]\nfn f(a: i64) -> i64 { if a > 5 { let m = a; } else { 0 } }\n",
        );
        let briev = &out[0].briev;
        assert!(briev.contains("when a <= 5 { term 0; };"), "{briev}");
    }

    #[test]
    fn let_binding_is_typed_int() {
        let out = translate("#[post(term == n + 1)]\nfn f(n: i64) -> i64 { let m = n + 1; m }\n");
        assert!(
            out[0].briev.contains("let m: Int = n + 1;"),
            "{}",
            out[0].briev
        );
    }

    #[test]
    fn uncontracted_functions_are_skipped() {
        assert!(translate("fn plain(a: i64) -> i64 { a }\n").is_empty());
    }

    #[test]
    fn non_i64_boundary_is_rejected() {
        let file = syn::parse_file("#[post(true)]\nfn f(a: u32) -> i64 { 0 }\n").unwrap();
        assert!(matches!(translate_file(&file), Err(Error::Unsupported(_))));
    }
}

#[cfg(test)]
mod contract_attribute_tests {
    use super::*;

    /// Parse Rust source and translate its contracted functions.
    fn translate(source: &str) -> (Vec<TranslatedFn>, Vec<String>) {
        let file = syn::parse_file(source).expect("parse rust");
        translate_file(&file).expect("translate")
    }

    /// Two attributes for one slot is an error, never a silent first-wins.
    #[test]
    fn duplicate_pre_is_rejected() {
        let file = syn::parse_file("#[pre(a > 0)]\n#[pre(a > 1)]\nfn f(a: i64) -> i64 { a }\n")
            .expect("parse");
        let err = translate_file(&file).expect_err("duplicate #[pre] must be rejected");
        assert!(err.to_string().contains("more than one"), "{err}");
    }

    /// The pre-rename spelling warns and names its replacement.
    #[test]
    fn renamed_attribute_warns_with_the_replacement() {
        let (_, warnings) =
            translate("#[requires(a > 0)]\n#[post(term >= 0)]\nfn f(a: i64) -> i64 { a }\n");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("renamed to `#[pre(...)]`"),
            "{warnings:?}"
        );
    }

    /// `#[pre]` alone is a valid contract; the postcondition stays optional.
    #[test]
    fn pre_only_is_a_contract() {
        let (out, warnings) = translate("#[pre(a > 0)]\nfn f(a: i64) -> i64 { a }\n");
        assert_eq!(out.len(), 1);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// Ordinary Rust attributes never produce a warning.
    #[test]
    fn ordinary_attributes_never_warn() {
        let (_, warnings) =
            translate("#[inline(always)]\n#[test]\n#[pre(a > 0)]\nfn f(a: i64) -> i64 { a }\n");
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}
