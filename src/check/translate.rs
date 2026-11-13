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

use super::Error;

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
    // P3: resolve calls BEFORE translation. The expression layer is deliberately
    // context-free, so threading a summary map through it would touch seven
    // signatures; inlining first keeps it a pure printer and costs one pass here.
    let summaries = build_summaries(&file.items);
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
        let inlined = inline_calls(item_fn, &summaries)?;
        let item_fn = &inlined;
        let translated_list = translate_fn(item_fn, pre, post)?;
        let translated = &translated_list[0];
        let name = translated.name.clone();
        let (params_text, _facts) = translate_params(&name, &item_fn.sig)?;
        let params: Vec<String> = item_fn
            .sig
            .inputs
            .iter()
            .filter_map(|arg| match arg {
                syn::FnArg::Typed(pat_ty) => match pat_ty.pat.as_ref() {
                    syn::Pat::Ident(pat_ident) => Some(pat_ident.ident.to_string()),
                    _ => None,
                },
                syn::FnArg::Receiver(_) => None,
            })
            .collect();
        // `#[case(...)]` instantiates the same contract at concrete inputs: a
        // parameterless defn the prover can decide outright, instead of leaving
        // the general goal `Unproven`.
        // A loop contract expands into several obligations whose signatures are
        // wider than the function's, so a case instantiated from the signature
        // alone would not match them. Only whole-function contracts are cased.
        let mut case_defns = Vec::new();
        if translated_list.len() == 1 {
            let cases = case_values_for(&item_fn.attrs, params.len(), &name)?;
            for (index, values) in cases.iter().enumerate() {
                case_defns.push(instantiate_case(
                    translated,
                    &name,
                    &params_text,
                    &params,
                    index,
                    values,
                )?);
            }
        }
        out.extend(translated_list);
        out.extend(case_defns);
    }
    Ok((out, warnings))
}

/// Translate one contracted function.
fn translate_fn(
    item_fn: &syn::ItemFn,
    pre: Option<String>,
    post: Option<String>,
) -> Result<Vec<TranslatedFn>, Error> {
    let name = item_fn.sig.ident.to_string();
    let (params, facts) = translate_params(&name, &item_fn.sig)?;
    let post = post.unwrap_or_else(|| "true".to_string());
    // Each narrower or unsigned parameter contributes a domain fact. Everything
    // crosses into Briev as one `Int`, so without these the prover would happily
    // consider values the Rust type forbids. A fact-free signature keeps the
    // author's precondition byte-for-byte: adding parentheses only matters when
    // facts are concatenated with `&&`.
    let pre = if facts.is_empty() {
        pre.unwrap_or_else(|| "true".to_string())
    } else {
        let mut pre_parts = Vec::new();
        if let Some(explicit) = pre.filter(|written| written != "true") {
            pre_parts.push(format!("({explicit})"));
        }
        pre_parts.extend(facts);
        pre_parts.join(" && ")
    };
    // P4 step (b): the invariant turns the loop into a straight-line
    // preservation obligation. Briev evaluates `[pre]` in the entry state and
    // `[post]` in the FINAL state, so `[I && i < hi][I]` is preservation itself —
    // no induction, no new prover machinery.
    if let Some(invariant) = contract_attr(&item_fn.attrs, "invariant")? {
        let contract = LoopContract {
            name: &name,
            params: &params,
            pre: &pre,
            invariant: &invariant,
            post: &post,
        };
        return translate_loop_contracts(&contract, item_fn);
    }
    let body = translate_block(&item_fn.block.stmts)?;
    if body.trim().is_empty() {
        return Err(Error::Unsupported(format!(
            "`{name}`: empty body has no value to verify"
        )));
    }
    let briev = format!("defn {name}({params}) [{pre}][{post}] -> Int {{\n    {body}\n}};\n");
    Ok(vec![TranslatedFn { name, briev }])
}

/// The integer width a boundary type names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntWidth {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    Usize,
    Isize,
}

/// Translate the parameter list, admitting every integer width Counsel models.
///
/// Returns the Briev parameter list plus the domain facts each parameter
/// contributes. Those facts are what make the single-`Int` model sound: the
/// parameter is carried as an `Int`, and the fact pins it to the range the Rust
/// type actually allows.
fn translate_params(name: &str, sig: &syn::Signature) -> Result<(String, Vec<String>), Error> {
    let mut params = Vec::new();
    let mut facts = Vec::new();
    for input in &sig.inputs {
        let syn::FnArg::Typed(pat_ty) = input else {
            return Err(Error::Unsupported(format!(
                "`{name}`: methods with `self` are not supported"
            )));
        };
        let width = integer_width(name, "parameter", &pat_ty.ty)?;
        let syn::Pat::Ident(pat_ident) = pat_ty.pat.as_ref() else {
            return Err(Error::Unsupported(format!(
                "`{name}`: only plain identifier parameters are supported"
            )));
        };
        let ident = pat_ident.ident.to_string();
        if let Some(fact) = domain_fact(&ident, width) {
            facts.push(fact);
        }
        params.push(format!("{ident}: Int"));
    }
    if let syn::ReturnType::Type(_, ty) = &sig.output {
        integer_width(name, "return type", ty)?;
    }
    Ok((params.join(", "), facts))
}

/// The integer width at a boundary, or a rejection naming what is accepted.
fn integer_width(name: &str, what: &str, ty: &syn::Type) -> Result<IntWidth, Error> {
    let syn::Type::Path(path) = ty else {
        return Err(not_integer(name, what));
    };
    let Some(last) = path.path.segments.last() else {
        return Err(not_integer(name, what));
    };
    Ok(match last.ident.to_string().as_str() {
        "i8" => IntWidth::I8,
        "i16" => IntWidth::I16,
        "i32" => IntWidth::I32,
        "i64" => IntWidth::I64,
        "u8" => IntWidth::U8,
        "u16" => IntWidth::U16,
        "u32" => IntWidth::U32,
        "u64" => IntWidth::U64,
        "usize" => IntWidth::Usize,
        "isize" => IntWidth::Isize,
        _ => return Err(not_integer(name, what)),
    })
}

/// The rejection for a non-integer boundary type.
fn not_integer(name: &str, what: &str) -> Error {
    Error::Unsupported(format!(
        "`{name}`: {what} must be an integer type (i8/i16/i32/i64, u8/u16/u32/u64, \
         usize/isize) — Counsel models integers as `Int`"
    ))
}

/// The range fact a parameter contributes, when its type is narrower than `Int`.
fn domain_fact(ident: &str, width: IntWidth) -> Option<String> {
    Some(match width {
        // Full-width signed types are already exactly the model.
        IntWidth::I64 | IntWidth::Isize => return None,
        IntWidth::I32 => format!("{ident} >= -2147483648 && {ident} <= 2147483647"),
        IntWidth::I16 => format!("{ident} >= -32768 && {ident} <= 32767"),
        IntWidth::I8 => format!("{ident} >= -128 && {ident} <= 127"),
        IntWidth::U8 => format!("{ident} >= 0 && {ident} <= 255"),
        IntWidth::U16 => format!("{ident} >= 0 && {ident} <= 65535"),
        IntWidth::U32 => format!("{ident} >= 0 && {ident} <= 4294967295"),
        // The type's own range exceeds `Int`. The model therefore covers the
        // non-negative `Int` range only — stated here rather than implied.
        IntWidth::U64 | IntWidth::Usize => format!("{ident} >= 0"),
    })
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
        // Loops are unrolled, never approximated: constant bounds only.
        syn::Expr::ForLoop(for_loop) => translate_for(for_loop, out),
        // Plain rebinding: `x = expr;` — the accumulator a loop needs.
        syn::Expr::Assign(assign) => translate_assign(assign, out),
        // `a.max(b)` in value position: Briev has no expression-level
        // conditional, so the value becomes guarded `term` statements.
        syn::Expr::MethodCall(call) if no_semi && is_last => match desugar_integer_method(call) {
            Some(method) => {
                emit_method_terms(&method, out);
                Ok(())
            }
            None => Err(Error::Unsupported(format!(
                "unsupported expression (pure-integer subset): {}",
                describe_expr(expr)
            ))),
        },
        // A trailing expression is the function's value.
        other if no_semi && is_last => {
            out.push(format!("term {};", translate_expr(other, true)?));
            Ok(())
        }
        other => Err(Error::Unsupported(format!(
            "unsupported statement (pure-integer subset): {}",
            describe_expr(other)
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
        other => value_statements(other),
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
/// Name the construct that fell outside the subset.
///
/// The message used to interpolate `{expr:?}`, which printed the whole syntax
/// tree — technically accurate and useless to a reader. Naming the construct
/// says what to change instead.
fn describe_expr(expr: &syn::Expr) -> &'static str {
    match expr {
        syn::Expr::MethodCall(_) => "a method call",
        syn::Expr::Call(_) => "a function call",
        syn::Expr::Field(_) => "field access",
        syn::Expr::Index(_) => "indexing",
        syn::Expr::Match(_) => "a `match` expression",
        syn::Expr::Closure(_) => "a closure",
        syn::Expr::Struct(_) => "a struct literal",
        syn::Expr::Array(_) => "an array literal",
        syn::Expr::Tuple(_) => "a tuple",
        syn::Expr::Reference(_) => "a reference",
        syn::Expr::Range(_) => "a range expression",
        syn::Expr::Cast(_) => "a cast",
        syn::Expr::Try(_) => "the `?` operator",
        syn::Expr::Await(_) => "`.await`",
        syn::Expr::Assign(_) => "an assignment in a position without a name to bind",
        other => {
            // Variants are listed explicitly where a reader would act on them;
            // the rest share one honest answer rather than an AST dump.
            let _ = other;
            "an expression outside the integer subset"
        }
    }
}

fn translate_expr(expr: &syn::Expr, allow_ident: bool) -> Result<String, Error> {
    match expr {
        syn::Expr::Lit(lit) => translate_lit(lit),
        syn::Expr::Path(path) => translate_path(path, allow_ident),
        syn::Expr::Binary(binary) => translate_binary(binary, allow_ident),
        syn::Expr::Unary(unary) => translate_unary(unary, allow_ident),
        syn::Expr::Paren(paren) => Ok(format!("({})", translate_expr(&paren.expr, allow_ident)?)),
        other => Err(Error::Unsupported(format!(
            "unsupported expression (pure-integer subset): {}",
            describe_expr(other)
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

/// Read every `#[case(...)]`, requiring one literal per parameter.
///
/// The attribute carries concrete *inputs* only; the expectation comes from the
/// function's own `#[post]`, so there is no second place to get it wrong.
fn case_values_for(
    attrs: &[syn::Attribute],
    param_count: usize,
    name: &str,
) -> Result<Vec<Vec<String>>, Error> {
    let mut out = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("case") {
            continue;
        }
        let expr: syn::Expr = attr
            .parse_args()
            .map_err(|e| Error::Parse(format!("`#[case(...)]`: {e}")))?;
        let values = case_literals(&expr)?;
        if values.len() != param_count {
            return Err(Error::Contract(format!(
                "`#[case(...)]` on `{name}` supplies {} value(s) but the function takes {param_count}",
                values.len()
            )));
        }
        out.push(values);
    }
    Ok(out)
}

/// Flatten one case's values: `#[case(5)]` or `#[case((2, 3))]`.
fn case_literals(expr: &syn::Expr) -> Result<Vec<String>, Error> {
    match expr {
        syn::Expr::Tuple(tuple) => tuple.elems.iter().map(case_literal).collect(),
        other => Ok(vec![case_literal(other)?]),
    }
}

/// A case value must be an integer literal, optionally negated.
fn case_literal(expr: &syn::Expr) -> Result<String, Error> {
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(int),
            ..
        }) => Ok(int.base10_digits().to_string()),
        syn::Expr::Unary(syn::ExprUnary {
            op: syn::UnOp::Neg(_),
            expr,
            ..
        }) => Ok(format!("-{}", case_literal(expr)?)),
        _ => Err(Error::Contract(
            "`#[case(...)]` values must be integer literals".to_string(),
        )),
    }
}

/// Materialise one case as a parameterless Briev defn.
///
/// Substitution is textual, so a body that rebinds a parameter (`let a = ...`)
/// is rejected rather than silently mis-substituted.
fn instantiate_case(
    base: &TranslatedFn,
    name: &str,
    params_text: &str,
    params: &[String],
    index: usize,
    values: &[String],
) -> Result<TranslatedFn, Error> {
    for param in params {
        if base.briev.contains(&format!("let {param}: Int")) {
            return Err(Error::Contract(format!(
                "`#[case(...)]` on `{name}` cannot be applied: the body rebinds `{param}`"
            )));
        }
    }
    let case_name = format!("{name}__case{index}");
    let renamed = base.briev.replacen(
        &format!("defn {name}({params_text})"),
        &format!("defn {case_name}()"),
        1,
    );
    if renamed == base.briev {
        return Err(Error::Contract(format!(
            "`#[case(...)]` on `{name}`: could not rewrite the emitted signature"
        )));
    }
    Ok(TranslatedFn {
        name: case_name,
        briev: substitute_identifiers(&renamed, params, values),
    })
}

/// Replace whole identifiers in emitted Briev text.
fn substitute_identifiers(text: &str, names: &[String], values: &[String]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch);
            continue;
        }
        flush_identifier(&mut out, &current, names, values);
        current.clear();
        out.push(ch);
    }
    flush_identifier(&mut out, &current, names, values);
    out
}

/// Emit one identifier, substituting it when it names a case parameter.
fn flush_identifier(out: &mut String, ident: &str, names: &[String], values: &[String]) {
    if ident.is_empty() {
        return;
    }
    match names.iter().position(|name| name == ident) {
        Some(pos) => out.push_str(&values[pos]),
        None => out.push_str(ident),
    }
}

#[cfg(test)]
mod case_tests {
    use super::*;

    /// Parse and translate, discarding the advisory warnings.
    fn translate(source: &str) -> Result<Vec<TranslatedFn>, Error> {
        let file = syn::parse_file(source).expect("parse rust");
        translate_file(&file).map(|(out, _)| out)
    }

    /// A case emits a parameterless, fully substituted defn next to the goal.
    #[test]
    fn case_emits_a_substituted_defn() {
        let out = translate(
            "#[pre(a >= 0)]\n#[post(term == a + a)]\n#[case(5)]\nfn double(a: i64) -> i64 { a + a }\n",
        )
        .expect("translate");
        assert_eq!(out.len(), 2, "general goal + one case");
        assert_eq!(out[0].name, "double");
        assert_eq!(out[1].name, "double__case0");
        assert!(
            out[1].briev.contains("defn double__case0()"),
            "{}",
            out[1].briev
        );
        assert!(out[1].briev.contains("term 5 + 5"), "{}", out[1].briev);
        assert!(!out[1].briev.contains("(a: Int)"), "{}", out[1].briev);
    }

    /// Several values map left-to-right; multi-argument cases use a tuple.
    #[test]
    fn multi_argument_case_maps_left_to_right() {
        let out = translate(
            "#[post(term == a + b)]\n#[case((2, 3))]\nfn add(a: i64, b: i64) -> i64 { a + b }\n",
        )
        .expect("translate");
        assert!(out[1].briev.contains("term 2 + 3"), "{}", out[1].briev);
    }

    /// A body that rebinds a parameter makes textual substitution unsound.
    #[test]
    fn shadowed_parameter_is_refused() {
        let err =
            translate("#[post(term >= 0)]\n#[case(5)]\nfn f(a: i64) -> i64 { let a = 1; a }\n")
                .expect_err("shadowing must be refused");
        assert!(err.to_string().contains("rebinds"), "{err}");
    }

    /// Wrong arity is a contract error, never a silent skip.
    #[test]
    fn wrong_arity_is_rejected() {
        let err = translate("#[post(term == a)]\n#[case((1, 2))]\nfn f(a: i64) -> i64 { a }\n")
            .expect_err("arity mismatch must be refused");
        assert!(
            err.to_string().contains("value(s) but the function takes"),
            "{err}"
        );
    }
}

#[cfg(test)]
mod width_tests {
    use super::*;

    /// M1: an unsigned parameter must carry the fact pinning it to its range.
    #[test]
    fn unsigned_parameter_carries_a_domain_fact() {
        let file =
            syn::parse_file("#[post(term >= 0)]\nfn f(x: u32) -> u32 { x }\n").expect("parse");
        let (out, _warnings) = translate_file(&file).expect("translate");
        assert!(
            out[0].briev.contains("x >= 0 && x <= 4294967295"),
            "u32 fact missing: {}",
            out[0].briev
        );
    }

    /// M1: a full-width parameter needs no fact - the model already matches it.
    #[test]
    fn i64_parameter_adds_no_domain_fact() {
        let file =
            syn::parse_file("#[post(term >= 0)]\nfn f(x: i64) -> i64 { x }\n").expect("parse");
        let (out, _warnings) = translate_file(&file).expect("translate");
        assert!(
            !out[0].briev.contains("x >= 0 && x <="),
            "i64 must add no fact: {}",
            out[0].briev
        );
    }
}

// ── P3: calls, resolved before translation ──────────────────────────────
//
// The expression layer is context-free on purpose, so a call is resolved HERE,
// on the `syn` tree, before any Briev text exists: a summarised call becomes the
// callee's value expression with the caller's arguments substituted in.

/// A callee's returned value, substitutable at a call site.
struct Summary {
    params: Vec<String>,
    value: syn::Expr,
}

/// Collect value summaries for pure integer functions with no precondition.
///
/// R3: substituting a callee's body assumes its precondition holds at the call
/// site, and this checker never establishes that — so a callee carrying an
/// explicit `#[pre]` earns no summary and its call stays unsupported. Anything
/// else would be a false proof resting on an unproven assumption.
///
/// R4: a body with an early `return` has no single value, so it is excluded too
/// (a `gcd`-style `if`/`else` *expression* is fine — it has one value).
fn build_summaries(items: &[syn::Item]) -> std::collections::HashMap<String, Summary> {
    let mut sums = std::collections::HashMap::new();
    for item in items {
        let syn::Item::Fn(item_fn) = item else {
            continue;
        };
        if matches!(contract_attr(&item_fn.attrs, "pre"), Ok(Some(_))) {
            continue;
        }
        let Some(params) = plain_integer_params(&item_fn.sig) else {
            continue;
        };
        let Some(value) = single_value(&item_fn.block) else {
            continue;
        };
        sums.insert(item_fn.sig.ident.to_string(), Summary { params, value });
    }
    sums
}

/// Parameter names when every parameter is a plain integer binding.
fn plain_integer_params(sig: &syn::Signature) -> Option<Vec<String>> {
    let mut params = Vec::new();
    for input in &sig.inputs {
        let syn::FnArg::Typed(pat_ty) = input else {
            return None;
        };
        let syn::Pat::Ident(ident) = pat_ty.pat.as_ref() else {
            return None;
        };
        integer_width("", "parameter", &pat_ty.ty).ok()?;
        params.push(ident.ident.to_string());
    }
    if let syn::ReturnType::Type(_, ty) = &sig.output {
        integer_width("", "return type", ty).ok()?;
    }
    Some(params)
}

/// The single value a summary-able body returns.
fn single_value(block: &syn::Block) -> Option<syn::Expr> {
    if block
        .stmts
        .iter()
        .any(|stmt| matches!(stmt, syn::Stmt::Expr(syn::Expr::Return(_), _)))
    {
        return None;
    }
    match block.stmts.last()? {
        syn::Stmt::Expr(expr, None) => Some(expr.clone()),
        _ => None,
    }
}

/// Resolve summarised calls to the callee's value before translation.
fn inline_calls(
    item_fn: &syn::ItemFn,
    sums: &std::collections::HashMap<String, Summary>,
) -> Result<syn::ItemFn, Error> {
    let mut inlined = item_fn.clone();
    let mut inliner = Inliner {
        sums,
        stack: Vec::new(),
        failure: None,
    };
    syn::visit_mut::VisitMut::visit_item_fn_mut(&mut inliner, &mut inlined);
    match inliner.failure {
        Some(error) => Err(error),
        None => Ok(inlined),
    }
}

/// Substitutes summary-able calls; leaves anything else for the translator.
struct Inliner<'a> {
    sums: &'a std::collections::HashMap<String, Summary>,
    stack: Vec<String>,
    failure: Option<Error>,
}

impl syn::visit_mut::VisitMut for Inliner<'_> {
    /// Rewrite one expression node, recursing first so nested calls resolve.
    fn visit_expr_mut(&mut self, node: &mut syn::Expr) {
        syn::visit_mut::visit_expr_mut(self, node);
        if self.failure.is_some() {
            return;
        }
        let syn::Expr::Call(call) = node else { return };
        let Some(name) = direct_name(&call.func) else {
            self.failure = Some(Error::Unsupported(
                "call through a non-name target is not supported".to_string(),
            ));
            return;
        };
        let Some(summary) = self.sums.get(&name) else {
            return;
        };
        if self.stack.iter().any(|seen| seen == &name) {
            // R4: a cycle earns no summary — leave the call for the translator.
            return;
        }
        if summary.params.len() != call.args.len() {
            self.failure = Some(Error::Unsupported(format!(
                "`{name}`: call arity does not match its definition"
            )));
            return;
        }
        let args: Vec<syn::Expr> = call.args.iter().cloned().collect();
        let mut value = summary.value.clone();
        let mut subst = Substitute {
            params: &summary.params,
            args: &args,
        };
        syn::visit_mut::VisitMut::visit_expr_mut(&mut subst, &mut value);
        self.stack.push(name);
        self.visit_expr_mut(&mut value);
        self.stack.pop();
        *node = value;
    }
}

/// The identifier a call target names, when it is a bare name.
fn direct_name(func: &syn::Expr) -> Option<String> {
    let syn::Expr::Path(path) = func else {
        return None;
    };
    path.path.get_ident().map(|ident| ident.to_string())
}

/// Replaces the callee's parameters with the caller's arguments.
struct Substitute<'a> {
    params: &'a [String],
    args: &'a [syn::Expr],
}

impl syn::visit_mut::VisitMut for Substitute<'_> {
    /// Swap a parameter reference for the argument it was called with.
    fn visit_expr_mut(&mut self, node: &mut syn::Expr) {
        if let Some(name) = direct_name(node) {
            if let Some(index) = self.params.iter().position(|param| param == &name) {
                *node = self.args[index].clone();
                return;
            }
        }
        syn::visit_mut::visit_expr_mut(self, node);
    }
}

#[cfg(test)]
mod call_tests {
    use super::*;

    /// A call to a summarised callee becomes the callee's value.
    #[test]
    fn call_to_a_summarised_callee_is_inlined() {
        let file = syn::parse_file(
            "fn inc(a: i64) -> i64 { a + 1 }\n\
             #[post(term == a + 2)]\n\
             fn uses(a: i64) -> i64 { inc(a) + 1 }\n",
        )
        .expect("parse");
        let (out, _warnings) = translate_file(&file).expect("translate");
        assert_eq!(out.len(), 1, "only the contracted fn is translated");
        assert!(
            !out[0].briev.contains("inc("),
            "call should be resolved, got: {}",
            out[0].briev
        );
    }

    /// R4: recursion is refused, never guessed at.
    #[test]
    fn recursion_is_refused_not_guessed() {
        let file = syn::parse_file(
            "#[post(term >= 0)]\nfn gcd(a: i64, b: i64) -> i64 { if b == 0 { a } else { gcd(b, a % b) } }\n",
        )
        .expect("parse");
        let err = translate_file(&file).expect_err("recursion must not produce a proof");
        assert!(matches!(err, Error::Unsupported(_)), "{err}");
    }

    /// R3: a callee carrying a real precondition is never summarised, because
    /// the call site never establishes it.
    #[test]
    fn preconditioned_callee_is_not_summarised() {
        let file = syn::parse_file(
            "#[pre(a > 0)]\nfn pos(a: i64) -> i64 { a }\n\
             #[post(term > 0)]\nfn uses(a: i64) -> i64 { pos(a) }\n",
        )
        .expect("parse");
        let err = translate_file(&file).expect_err("preconditioned callee must stay unsupported");
        assert!(matches!(err, Error::Unsupported(_)), "{err}");
    }
}

/// Largest constant loop that will be unrolled.
const MAX_LOOP_UNROLL: i64 = 64;

/// Translate a `for` loop by fully unrolling it.
///
/// Unrolling IS the literal semantics of a constant-bounded loop, so it stays
/// sound: nothing is assumed about a trip count. Dynamic bounds are refused
/// rather than approximated, because any summary of them would be a guess.
fn translate_for(for_loop: &syn::ExprForLoop, out: &mut Vec<String>) -> Result<(), Error> {
    let syn::Pat::Ident(pat) = for_loop.pat.as_ref() else {
        return Err(Error::Unsupported(
            "`for` loops are supported only with a plain loop variable".to_string(),
        ));
    };
    let syn::Expr::Range(range) = for_loop.expr.as_ref() else {
        return Err(Error::Unsupported(
            "`for` loops are supported only over `lo..hi` / `lo..=hi`".to_string(),
        ));
    };
    let (Some(start), Some(end)) = (
        range.start.as_deref().and_then(literal_int),
        range.end.as_deref().and_then(literal_int),
    ) else {
        return Err(Error::Unsupported(
            "`for` loop bounds must be integer literals: a dynamic bound (e.g. `0..n`) \
             needs a loop invariant, which Counsel does not support yet. Unrolling a \
             variable trip count would be unsound, so it is refused rather than \
             approximated. Extract the loop into a constant-bound form, or verify its \
             caller instead"
                .to_string(),
        ));
    };
    let end = match range.limits {
        syn::RangeLimits::HalfOpen(_) => end,
        syn::RangeLimits::Closed(_) => end + 1,
    };
    let count = end.saturating_sub(start);
    if count > MAX_LOOP_UNROLL {
        return Err(Error::Unsupported(format!(
            "`for` loop of {count} iterations exceeds the unrolling limit ({MAX_LOOP_UNROLL})"
        )));
    }
    if count <= 0 {
        return Ok(());
    }
    let name = pat.ident.to_string();
    for step in start..end {
        let index: syn::Expr = syn::parse_str(&step.to_string())
            .map_err(|_| Error::Unsupported("loop index literal".to_string()))?;
        let mut body = for_loop.body.clone();
        let mut swap = Substitute {
            params: std::slice::from_ref(&name),
            args: std::slice::from_ref(&index),
        };
        for stmt in &mut body.stmts {
            syn::visit_mut::VisitMut::visit_stmt_mut(&mut swap, stmt);
        }
        let text = translate_block(&body.stmts)?;
        if !text.trim().is_empty() {
            out.push(text);
        }
    }
    Ok(())
}

/// An integer literal, allowing a leading negation.
fn literal_int(expr: &syn::Expr) -> Option<i64> {
    match expr {
        syn::Expr::Lit(lit) => match &lit.lit {
            syn::Lit::Int(int) => int.base10_parse().ok(),
            _ => None,
        },
        syn::Expr::Unary(unary) => match unary.op {
            syn::UnOp::Neg(_) => literal_int(&unary.expr).map(|value| -value),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod loop_tests {
    use super::*;

    /// The loop arm is reached: constant bounds unroll, dynamic bounds refuse.
    #[test]
    fn dynamic_loop_bounds_are_refused_not_approximated() {
        let file = syn::parse_file(
            "#[post(term >= 0)]\nfn f(a: i64) -> i64 { for i in 0..a { let t = i; } a }\n",
        )
        .expect("parse");
        let err = translate_file(&file).expect_err("dynamic bounds must be refused");
        let text = format!("{err}");
        assert!(
            text.contains("bounds must be integer literals"),
            "expected the unroller's refusal, got: {text}"
        );
    }

    /// A constant-bounded loop is expanded, leaving no `for` in the Briev text.
    #[test]
    fn constant_loop_is_unrolled() {
        let file = syn::parse_file(
            "#[post(term >= 0)]\nfn f(a: i64) -> i64 { for i in 0..2 { let t = i; } a }\n",
        )
        .expect("parse");
        let (out, _warnings) = translate_file(&file).expect("translate");
        assert!(
            !out[0].briev.contains("for "),
            "loop should be expanded: {}",
            out[0].briev
        );
    }

    /// Loop + accumulator now translates. This test previously asserted the
    /// opposite: unrolling alone was inert because assignment was outside the
    /// subset, so an expanded loop could not accumulate. Adding rebinding
    /// closed that gap, and the assertion flipped with it.
    #[test]
    fn assignment_inside_a_loop_translates() {
        let file = syn::parse_file(
            "#[post(term == 1)]\nfn f() -> i64 { let mut acc = 0; for i in 0..2 { acc = acc + i; } acc }\n",
        )
        .expect("parse");
        let (out, _warnings) = translate_file(&file).expect("translate");
        assert!(
            out[0].briev.contains("acc = acc + 0") && out[0].briev.contains("acc = acc + 1"),
            "expected both unrolled rebindings, got: {}",
            out[0].briev
        );
    }
}

/// Translate a plain rebinding: `x = expr;`.
///
/// Only a bare-name target is supported. A field or index target would need a
/// memory model this checker does not have, so it is refused rather than
/// approximated.
fn translate_assign(assign: &syn::ExprAssign, out: &mut Vec<String>) -> Result<(), Error> {
    let syn::Expr::Path(path) = assign.left.as_ref() else {
        return Err(Error::Unsupported(
            "assignment target must be a plain name".to_string(),
        ));
    };
    let Some(ident) = path.path.get_ident() else {
        return Err(Error::Unsupported(
            "assignment target must be a plain name".to_string(),
        ));
    };
    if let syn::Expr::MethodCall(call) = assign.right.as_ref() {
        let method = desugar_integer_method(call).ok_or_else(|| {
            Error::Unsupported(format!(
                "unsupported expression (pure-integer subset): {}",
                describe_expr(&assign.right)
            ))
        })?;
        emit_method_assign(&method, &ident.to_string(), out);
        return Ok(());
    }
    let value = translate_expr(&assign.right, true)?;
    out.push(format!("{ident} = {value};"));
    Ok(())
}

/// An integer method call expanded into its branch structure.
enum IntegerMethod {
    /// `a.max(b)`
    Max(String, String),
    /// `a.min(b)`
    Min(String, String),
    /// `a.abs()`
    Abs(String),
}

/// Translate a value expression into statements, desugaring supported integer
/// method calls into guarded `term`s.
///
/// Positions that build text directly — a loop-exit tail, an `if` branch value —
/// do not pass through the statement translator, so they need this.
fn value_statements(expr: &syn::Expr) -> Result<String, Error> {
    if let syn::Expr::MethodCall(call) = expr {
        let method = desugar_integer_method(call).ok_or_else(|| {
            Error::Unsupported(format!(
                "unsupported expression (pure-integer subset): {}",
                describe_expr(expr)
            ))
        })?;
        let mut out = Vec::new();
        emit_method_terms(&method, &mut out);
        return Ok(out.join("\n    "));
    }
    Ok(format!("term {};", translate_expr(expr, true)?))
}

/// Recognise `a.max(b)`, `a.min(b)` and `a.abs()`, and only those arities.
///
/// Briev has no expression-level conditional, and its verifier silently skips
/// any body containing a call — so a helper `defn` cannot express these. The call
/// has to become `when` statements here, which is the machinery the rest of the
/// translator already discharges.
fn desugar_integer_method(call: &syn::ExprMethodCall) -> Option<IntegerMethod> {
    let receiver = translate_expr(&call.receiver, true).ok()?;
    let argument = || translate_expr(&call.args[0], true).ok();
    match (call.method.to_string().as_str(), call.args.len()) {
        ("max", 1) => Some(IntegerMethod::Max(receiver, argument()?)),
        ("min", 1) => Some(IntegerMethod::Min(receiver, argument()?)),
        ("abs", 0) => Some(IntegerMethod::Abs(receiver)),
        _ => None,
    }
}

/// Emit the value of a recognised method call as guarded `term` statements.
fn emit_method_terms(method: &IntegerMethod, out: &mut Vec<String>) {
    let text = match method {
        IntegerMethod::Max(a, b) => format!("when {a} >= {b} {{ term {a}; }};\n    term {b};"),
        IntegerMethod::Min(a, b) => format!("when {a} <= {b} {{ term {a}; }};\n    term {b};"),
        IntegerMethod::Abs(a) => format!("when {a} >= 0 {{ term {a}; }};\n    term 0 - {a};"),
    };
    out.push(text);
}

/// Emit a recognised method call assigned to a plain name.
fn emit_method_assign(method: &IntegerMethod, target: &str, out: &mut Vec<String>) {
    let text = match method {
        IntegerMethod::Max(a, b) => {
            format!("{target} = {a};\n    when {b} > {a} {{ {target} = {b}; }};")
        }
        IntegerMethod::Min(a, b) => {
            format!("{target} = {a};\n    when {b} < {a} {{ {target} = {b}; }};")
        }
        IntegerMethod::Abs(a) => {
            format!("{target} = {a};\n    when {a} < 0 {{ {target} = 0 - {a}; }};")
        }
    };
    out.push(text);
}

#[cfg(test)]
mod method_call_tests {
    use super::*;

    /// Translate one function and return its emitted Briev text.
    fn briev_for(source: &str) -> String {
        let file = syn::parse_file(source).expect("parse");
        let (out, _warnings) = translate_file(&file).expect("translate");
        out[0].briev.clone()
    }

    /// `max` becomes a guarded `term`, because Briev has no expression-level
    /// conditional and a helper defn containing a call is skipped by the prover.
    #[test]
    fn max_in_value_position_becomes_guarded_terms() {
        let briev = briev_for("#[post(term >= 0)]\nfn f(a: i64, b: i64) -> i64 { a.max(b) }\n");
        assert!(briev.contains("when a >= b { term a; }"), "{briev}");
        assert!(briev.contains("term b;"), "{briev}");
    }

    /// `min` assigned to a name becomes an unconditional bind plus a guarded one.
    #[test]
    fn min_in_assignment_position_becomes_a_guarded_rebinding() {
        let briev = briev_for(
            "#[post(term >= 0)]\nfn f(a: i64, b: i64) -> i64 { let mut x = 0; x = a.min(b); x }\n",
        );
        assert!(briev.contains("x = a;"), "{briev}");
        assert!(briev.contains("when b < a { x = b; }"), "{briev}");
    }

    /// `abs` needs no argument and splits on the sign.
    #[test]
    fn abs_becomes_a_sign_split() {
        let briev = briev_for("#[post(term >= 0)]\nfn f(a: i64) -> i64 { a.abs() }\n");
        assert!(briev.contains("when a >= 0 { term a; }"), "{briev}");
        assert!(briev.contains("term 0 - a;"), "{briev}");
    }

    /// A loop-exit tail reaches the desugaring too, even though it is built as
    /// text rather than through the statement translator.
    #[test]
    fn loop_exit_tail_desugars_a_method_call() {
        let file = syn::parse_file(
            "#[post(term >= 1)]\n#[invariant(best >= 1)]\nfn f(len: usize) -> usize { let mut best = 1; let half = len / 2; for d in 1..=half { if len % d == 0 { best = d; } } best.max(1) }\n",
        )
        .expect("parse");
        let (out, _warnings) = translate_file(&file).expect("translate");
        let exit = out
            .iter()
            .find(|defn| defn.name == "f_exit")
            .expect("exit obligation");
        assert!(
            exit.briev.contains("when best >= 1 { term best; }"),
            "{}",
            exit.briev
        );
    }

    /// A method outside the whitelist is still refused, and named.
    #[test]
    fn unsupported_method_is_refused_by_name() {
        let file = syn::parse_file("#[post(term >= 0)]\nfn f(a: i64) -> i64 { a.pow(2) }\n")
            .expect("parse");
        let err = translate_file(&file).expect_err("pow is outside the subset");
        let text = format!("{err}");
        assert!(text.contains("a method call"), "{text}");
        assert!(
            !text.contains("Expr::MethodCall"),
            "must not dump the AST: {text}"
        );
    }
}

#[cfg(test)]
mod assign_tests {
    use super::*;

    /// A plain rebinding becomes a Briev assignment.
    #[test]
    fn assignment_becomes_a_briev_rebinding() {
        let file = syn::parse_file(
            "#[post(term == 5)]\nfn f() -> i64 { let mut acc = 0; acc = acc + 5; acc }\n",
        )
        .expect("parse");
        let (out, _warnings) = translate_file(&file).expect("translate");
        assert!(
            out[0].briev.contains("acc = acc + 5"),
            "expected a rebinding, got: {}",
            out[0].briev
        );
    }

    /// A parameter is a plain name, so rebinding it translates too.
    #[test]
    fn parameter_rebinding_is_a_plain_name() {
        let file = syn::parse_file("#[post(term >= 0)]\nfn f(p: i64) -> i64 { p = 1; p }\n")
            .expect("parse");
        let (out, _warnings) = translate_file(&file).expect("translate");
        assert!(out[0].briev.contains("p = 1"), "{}", out[0].briev);
    }

    /// An index target needs a memory model this checker does not have.
    #[test]
    fn assignment_to_an_index_target_is_refused() {
        let file = syn::parse_file("#[post(term >= 0)]\nfn f(a: i64) -> i64 { a[0] = 1; a }\n")
            .expect("parse");
        let err = translate_file(&file).expect_err("index target must be refused");
        assert!(format!("{err}").contains("plain name"), "{err}");
    }
}

#[cfg(test)]
mod invariant_tests {
    use super::*;

    /// The annotation is recognised, and refusing it says why rather than
    /// pretending the contract was checked.
    #[test]
    fn invariant_is_recognised_and_refused_precisely() {
        let file = syn::parse_file(
            "#[post(term >= 0)]\n#[invariant(acc >= 0)]\nfn f(n: i64) -> i64 { let mut acc = 0; for i in 0..n { acc = acc + i; } acc }\n",
        )
        .expect("parse");
        let (out, _warnings) = translate_file(&file).expect("preservation is emitted");
        let names: Vec<&str> = out.iter().map(|defn| defn.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["f_init", "f_preserve", "f_exit"],
            "a loop is only proved when all three obligations are emitted"
        );
        let preserve = out
            .iter()
            .find(|d| d.name == "f_preserve")
            .expect("preserve");
        assert!(
            preserve
                .briev
                .contains("[acc >= 0 && i >= 0 && i < n][acc >= 0]"),
            "expected the preservation contract, got: {}",
            preserve.briev
        );
    }

    /// Each obligation carries the antecedent it needs, and neither the entry
    /// nor the exit condition assumes the invariant where it must be proved.
    #[test]
    fn loop_contracts_carry_entry_and_exit_antecedents() {
        let file = syn::parse_file(
            "#[pre(n >= 0 && n <= 1000)]\n#[post(term >= 0)]\n#[invariant(acc == i && i <= n)]\nfn count(n: i64) -> i64 { let mut acc = 0; for i in 0..n { acc = acc + 1; } acc }\n",
        )
        .expect("parse");
        let (out, _warnings) = translate_file(&file).expect("emits three obligations");
        let find = |wanted: &str| {
            out.iter()
                .find(|defn| defn.name == wanted)
                .unwrap_or_else(|| panic!("missing {wanted}"))
                .briev
                .clone()
        };
        let init = find("count_init");
        assert!(
            init.contains("[n >= 0 && n <= 1000 && acc == 0 && i == 0][acc == i && i <= n]"),
            "entry must pin locals to their initialisers, not assume the invariant: {init}"
        );
        let preserve = find("count_preserve");
        assert!(
            preserve.contains("[n >= 0 && n <= 1000 && acc == i && i <= n && i >= 0 && i < n]"),
            "preservation must carry the precondition and the loop guard: {preserve}"
        );
        let exit = find("count_exit");
        assert!(
            exit.contains("[n >= 0 && n <= 1000 && acc == i && i <= n && i >= n][term >= 0]"),
            "exit must add the negation of the guard: {exit}"
        );
        assert!(
            exit.contains("term acc;"),
            "term must be the returned value: {exit}"
        );
    }

    /// A loop body usually guards its update, and its bounds often name a local.
    /// Both must reach the obligation's parameters: otherwise the emitted defn
    /// names an undefined variable and the obligation is lost as a type error.
    #[test]
    fn guarded_update_and_bound_local_are_carried() {
        let file = syn::parse_file(
            "#[post(term >= 1)]\n#[invariant(best >= 1)]\nfn step(len: usize) -> usize { let mut best = 1; let half = len / 2; for d in 1..=half { if len % d == 0 { best = d; } } best }\n",
        )
        .expect("parse");
        let (out, _warnings) = translate_file(&file).expect("emits the obligations");
        let find = |wanted: &str| {
            out.iter()
                .find(|defn| defn.name == wanted)
                .unwrap_or_else(|| panic!("missing {wanted}"))
                .briev
                .clone()
        };
        let preserve = find("step_preserve");
        assert!(
            preserve.contains("best: Int"),
            "an assignment inside `if` must still be a parameter: {preserve}"
        );
        assert!(
            preserve.contains("half: Int"),
            "the loop bound names `half`, so it must be a parameter: {preserve}"
        );
        let init = find("step_init");
        assert!(init.contains("best == 1"), "entry must pin best: {init}");
        assert!(
            init.contains("half == len / 2"),
            "entry must pin the bound local: {init}"
        );
    }

    /// A repeated invariant is rejected, like a repeated `#[pre]` slot.
    #[test]
    fn duplicate_invariant_is_rejected() {
        let file = syn::parse_file(
            "#[post(term >= 0)]\n#[invariant(acc >= 0)]\n#[invariant(acc > 0)]\nfn f(n: i64) -> i64 { n }\n",
        )
        .expect("parse");
        let err = translate_file(&file).expect_err("duplicate slots must be rejected");
        assert!(format!("{err}").contains("more than one"), "{err}");
    }
}

/// The parameter a loop contract varies over, bundled so the obligation
/// builders stay under the parameter limit rather than growing an argument list.
struct LoopContract<'a> {
    name: &'a str,
    params: &'a str,
    pre: &'a str,
    invariant: &'a str,
    post: &'a str,
}

/// The shape of the single `for` loop an `#[invariant(...)]` applies to.
struct LoopShape {
    var: String,
    lower: String,
    bound: String,
    closed: bool,
    at: usize,
    body: Vec<syn::Stmt>,
    tail: String,
}

/// Locate the loop an invariant applies to, plus the value returned after it.
///
/// `post` is stated in terms of `term`, so VC3 needs the trailing expression to
/// know what `term` becomes at exit.
fn loop_shape(item_fn: &syn::ItemFn) -> Result<LoopShape, Error> {
    for (at, stmt) in item_fn.block.stmts.iter().enumerate() {
        let syn::Stmt::Expr(syn::Expr::ForLoop(for_loop), _) = stmt else {
            continue;
        };
        let syn::Pat::Ident(pat) = for_loop.pat.as_ref() else {
            continue;
        };
        let syn::Expr::Range(range) = for_loop.expr.as_ref() else {
            continue;
        };
        let (Some(start), Some(end)) = (range.start.as_deref(), range.end.as_deref()) else {
            continue;
        };
        let tail = item_fn
            .block
            .stmts
            .iter()
            .skip(at + 1)
            .rev()
            .find_map(|later| match later {
                syn::Stmt::Expr(expr, None) => Some(expr),
                _ => None,
            })
            .ok_or_else(|| {
                Error::Unsupported(
                    "`#[invariant(...)]` needs the loop followed by a returned value, so the exit state can be checked against the postcondition"
                        .to_string(),
                )
            })?;
        return Ok(LoopShape {
            var: pat.ident.to_string(),
            lower: translate_expr(start, true)?,
            bound: translate_expr(end, true)?,
            closed: matches!(range.limits, syn::RangeLimits::Closed(_)),
            at,
            body: for_loop.body.stmts.clone(),
            tail: value_statements(tail)?,
        });
    }
    Err(Error::Unsupported(
        "`#[invariant(...)]` needs a `for` loop over `lo..hi` to apply to".to_string(),
    ))
}

/// The parameter list every obligation shares: the function's own parameters,
/// each local the loop writes, and the loop variable itself.
///
/// A written local must be carried in, or the invariant would be
/// under-specified and preservation would prove nothing.
fn loop_params(params: &str, mutated: &[String], var: &str) -> String {
    let mut parts: Vec<String> = if params.trim().is_empty() {
        Vec::new()
    } else {
        params.split(", ").map(str::to_string).collect()
    };
    for local in mutated {
        if !parts
            .iter()
            .any(|part| part.starts_with(&format!("{local}:")))
        {
            parts.push(format!("{local}: Int"));
        }
    }
    parts.push(format!("{var}: Int"));
    parts.join(", ")
}

/// VC1 pins each written local to the value it enters the loop with.
///
/// A local initialised after the loop, or not at all, contributes no pin; the
/// entry obligation is then simply left unproven rather than assumed.
fn entry_pins(
    item_fn: &syn::ItemFn,
    at: usize,
    mutated: &[String],
    var: &str,
    lower: &str,
) -> Result<String, Error> {
    let mut pins: Vec<String> = Vec::new();
    for stmt in item_fn.block.stmts.iter().take(at) {
        let syn::Stmt::Local(local) = stmt else {
            continue;
        };
        let (syn::Pat::Ident(pat), Some(init)) = (&local.pat, local.init.as_ref()) else {
            continue;
        };
        let local_name = pat.ident.to_string();
        if !mutated.contains(&local_name) {
            continue;
        }
        pins.push(format!(
            "{local_name} == {}",
            translate_expr(&init.expr, true)?
        ));
    }
    pins.push(format!("{var} == {lower}"));
    Ok(pins.join(" && "))
}

/// Emit the three loop obligations for an invariant-carrying loop.
///
/// Hoare's while-rule reduces a loop to three straight-line conditions that the
/// existing translate -> verify-bodies -> solver path discharges with no
/// induction:
///
///   VC1  pre && locals=init && i=lo  =>  I      the invariant holds at entry
///   VC2  pre && I && i in range      =>  I      the body preserves it
///   VC3  pre && I && i past the end  =>  post   the exit state meets the contract
///
/// All three are required. VC2 alone says only that the invariant survives one
/// step; it says nothing about where it starts or what it is worth at exit, so on
/// its own it proves nothing about the function. A wrong invariant is caught at
/// the obligation where it actually fails, which is why a loop that proves on
/// all three is proved and one that proves only some is not.
fn translate_loop_contracts(
    contract: &LoopContract<'_>,
    item_fn: &syn::ItemFn,
) -> Result<Vec<TranslatedFn>, Error> {
    let LoopContract {
        name,
        params,
        pre,
        invariant,
        post,
    } = *contract;
    let shape = loop_shape(item_fn)?;
    let locals = loop_locals(item_fn, &shape);
    let all = loop_params(params, &locals, &shape.var);
    let pins = entry_pins(item_fn, shape.at, &locals, &shape.var, &shape.lower)?;
    let body_text = translate_block(&shape.body)?;
    let (var, lower, bound) = (&shape.var, &shape.lower, &shape.bound);
    // `lo..hi` stops at `i == hi`; `lo..=hi` runs one step further.
    let (within, past) = if shape.closed {
        (format!("{var} <= {bound}"), format!("{var} > {bound}"))
    } else {
        (format!("{var} < {bound}"), format!("{var} >= {bound}"))
    };
    // The function's precondition holds at loop entry, so it belongs in every
    // antecedent: a bound like `n <= 1000` is what makes the arithmetic
    // wraparound-free, and without it nothing bounded can be proved.
    // No parentheses: Briev's parser normalises them away (its `Expr` has no
    // paren node), so emitting them would add noise to the text without changing
    // the meaning.
    let pre_text = if pre == "true" {
        String::new()
    } else {
        format!("{pre} && ")
    };
    let init = format!(
        "defn {name}_init({all}) [{pre_text}{pins}][{invariant}] -> Int {{\n    term 0;\n}};\n"
    );
    let preserve = format!(
        "defn {name}_preserve({all}) [{pre_text}{invariant} && {var} >= {lower} && {within}][{invariant}] -> Int {{\n    {body_text}\n    {var} = {var} + 1;\n    term 0;\n}};\n"
    );
    let exit = format!(
        "defn {name}_exit({all}) [{pre_text}{invariant} && {past}][{post}] -> Int {{\n    {}\n}};\n",
        shape.tail
    );
    Ok(vec![
        TranslatedFn {
            name: format!("{name}_init"),
            briev: init,
        },
        TranslatedFn {
            name: format!("{name}_preserve"),
            briev: preserve,
        },
        TranslatedFn {
            name: format!("{name}_exit"),
            briev: exit,
        },
    ])
}

/// Every local a loop obligation must carry as a parameter.
///
/// Two sources, because both are needed and neither implies the other:
///   * `let` bindings made before the loop, which supply the loop bounds
///     (`1..=half`) and the entry values VC1 pins;
///   * anything the body assigns, which may be nested inside a guard.
///
/// Missing either yields a defn naming an undefined variable — a type error
/// rather than a wrong verdict, but the obligation is lost either way.
fn loop_locals(item_fn: &syn::ItemFn, shape: &LoopShape) -> Vec<String> {
    let mut locals: Vec<String> = Vec::new();
    for stmt in item_fn.block.stmts.iter().take(shape.at) {
        let syn::Stmt::Local(local) = stmt else {
            continue;
        };
        let syn::Pat::Ident(pat) = &local.pat else {
            continue;
        };
        let name = pat.ident.to_string();
        if !locals.contains(&name) {
            locals.push(name);
        }
    }
    for assigned in assigned_names(&shape.body) {
        if !locals.contains(&assigned) {
            locals.push(assigned);
        }
    }
    locals
}

/// Names assigned by a statement list, including assignments nested inside
/// `if`/`else`, blocks, and inner `for`/`while`/`loop` bodies.
fn assigned_names(stmts: &[syn::Stmt]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    collect_assigned(stmts, &mut out);
    out
}

/// Recursive worker for [`assigned_names`], recording each distinct target once.
fn collect_assigned(stmts: &[syn::Stmt], out: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            syn::Stmt::Expr(expr, _) => collect_assigned_expr(expr, out),
            syn::Stmt::Local(local) => {
                if let Some(init) = local.init.as_ref() {
                    collect_assigned_expr(&init.expr, out);
                }
            }
            _ => {}
        }
    }
}

/// Record the target of an assignment wherever it appears in an expression.
fn collect_assigned_expr(expr: &syn::Expr, out: &mut Vec<String>) {
    match expr {
        syn::Expr::Assign(assign) => {
            if let syn::Expr::Path(path) = assign.left.as_ref() {
                if let Some(ident) = path.path.get_ident() {
                    let name = ident.to_string();
                    if !out.contains(&name) {
                        out.push(name);
                    }
                }
            }
            collect_assigned_expr(&assign.right, out);
        }
        syn::Expr::If(if_expr) => {
            collect_assigned(&if_expr.then_branch.stmts, out);
            if let Some((_, else_expr)) = if_expr.else_branch.as_ref() {
                collect_assigned_expr(else_expr, out);
            }
        }
        syn::Expr::Block(block) => collect_assigned(&block.block.stmts, out),
        syn::Expr::ForLoop(for_loop) => collect_assigned(&for_loop.body.stmts, out),
        syn::Expr::While(while_loop) => collect_assigned(&while_loop.body.stmts, out),
        syn::Expr::Loop(loop_expr) => collect_assigned(&loop_expr.body.stmts, out),
        syn::Expr::Binary(binary) => {
            collect_assigned_expr(&binary.left, out);
            collect_assigned_expr(&binary.right, out);
        }
        _ => {}
    }
}
