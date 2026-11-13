// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// build.rs (counsel) — construct `briev_verify` AST nodes from Rust syntax.
//
// This replaces the text emitter. Instead of printing Briev source, writing it
// to disk and shelling out to a compiler, counsel builds the nodes the prover
// walks and calls it in-process. No parser, no subprocess, no pinned toolchain.
//
// The trade is that the compiler's typechecker no longer sees the result, so the
// emitter must be right by construction rather than by validation.

use super::Error;
use briev_verify::ast::{
    BinaryOpKind, Contract, Definition, Expr, Statement, TopLevel, UnaryOpKind,
};

/// An integer literal node.
fn lit(value: i64) -> Expr {
    Expr::Decimal(value)
}

/// A bare identifier node.
fn ident(name: &str) -> Expr {
    Expr::Identifier(name.to_string())
}

/// A binary operation node.
fn binary(kind: BinaryOpKind, left: Expr, right: Expr) -> Expr {
    Expr::BinaryOp(kind, Box::new(left), Box::new(right))
}

/// `left && right`.
fn and(left: Expr, right: Expr) -> Expr {
    binary(BinaryOpKind::And, left, right)
}

/// Conjoin every expression, left to right. `None` when the list is empty.
fn conjunction(mut parts: Vec<Expr>) -> Option<Expr> {
    let mut combined = parts.pop()?;
    while let Some(part) = parts.pop() {
        combined = and(part, combined);
    }
    Some(combined)
}

/// The integer width a boundary type names, or `None` when it is not an integer.
fn width_of(ty: &syn::Type) -> Option<&'static str> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let ident = path.path.segments.last()?.ident.to_string();
    Some(match ident.as_str() {
        "i8" => "i8",
        "i16" => "i16",
        "i32" => "i32",
        "i64" => "i64",
        "isize" => "isize",
        "u8" => "u8",
        "u16" => "u16",
        "u32" => "u32",
        "u64" => "u64",
        "usize" => "usize",
        _ => return None,
    })
}

/// The range fact a narrower or unsigned parameter contributes.
///
/// Everything crosses into the prover as one `Int`, so without these the solver
/// would consider values the Rust type forbids. Must match the text emitter
/// exactly, or the two paths would not be interchangeable.
fn domain_fact(name: &str, width: &str) -> Option<Expr> {
    let range = |lo: i64, hi: i64| {
        and(
            binary(BinaryOpKind::Ge, ident(name), lit(lo)),
            binary(BinaryOpKind::Le, ident(name), lit(hi)),
        )
    };
    Some(match width {
        // Full-width signed types are already exactly the model.
        "i64" | "isize" => return None,
        "i32" => range(-2147483648, 2147483647),
        "i16" => range(-32768, 32767),
        "i8" => range(-128, 127),
        "u8" => range(0, 255),
        "u16" => range(0, 65535),
        "u32" => range(0, 4294967295),
        // The type's own range exceeds `Int`. The model therefore covers the
        // non-negative `Int` range only — stated rather than implied.
        "u64" | "usize" => binary(BinaryOpKind::Ge, ident(name), lit(0)),
        _ => return None,
    })
}

/// Name the construct that fell outside the subset, so the error says what to
/// change instead of dumping a syntax tree.
fn describe(expr: &syn::Expr) -> String {
    match expr {
        syn::Expr::MethodCall(_) => "a method call".to_string(),
        syn::Expr::Call(_) => "a function call".to_string(),
        syn::Expr::Field(_) => "field access".to_string(),
        syn::Expr::Index(_) => "indexing".to_string(),
        syn::Expr::Match(_) => "a `match` expression".to_string(),
        syn::Expr::Closure(_) => "a closure".to_string(),
        syn::Expr::Struct(_) => "a struct literal".to_string(),
        syn::Expr::Try(_) => "the `?` operator".to_string(),
        _ => "an expression outside the integer subset".to_string(),
    }
}

/// Map a Rust binary operator onto the prover's operator kind.
fn binary_kind(op: &syn::BinOp) -> Option<BinaryOpKind> {
    Some(match op {
        syn::BinOp::Add(_) => BinaryOpKind::Add,
        syn::BinOp::Sub(_) => BinaryOpKind::Sub,
        syn::BinOp::Mul(_) => BinaryOpKind::Mul,
        syn::BinOp::Div(_) => BinaryOpKind::Div,
        syn::BinOp::Rem(_) => BinaryOpKind::Mod,
        syn::BinOp::Eq(_) => BinaryOpKind::Eq,
        syn::BinOp::Ne(_) => BinaryOpKind::Neq,
        syn::BinOp::Lt(_) => BinaryOpKind::Lt,
        syn::BinOp::Le(_) => BinaryOpKind::Le,
        syn::BinOp::Gt(_) => BinaryOpKind::Gt,
        syn::BinOp::Ge(_) => BinaryOpKind::Ge,
        syn::BinOp::And(_) => BinaryOpKind::And,
        syn::BinOp::Or(_) => BinaryOpKind::Or,
        _ => return None,
    })
}

/// Build a value expression.
fn expr(node: &syn::Expr, allow_ident: bool) -> Result<Expr, Error> {
    match node {
        syn::Expr::Lit(lit_expr) => match &lit_expr.lit {
            syn::Lit::Int(value) => {
                let digits = value.base10_digits().replace('_', "");
                digits.parse::<i64>().map(lit).map_err(|_| {
                    Error::Unsupported(format!("integer literal out of range: {digits}"))
                })
            }
            other => Err(Error::Unsupported(format!(
                "unsupported literal (integers only): {other:?}"
            ))),
        },
        syn::Expr::Path(path) => {
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
            Ok(ident(&segment.ident.to_string()))
        }
        syn::Expr::Binary(bin) => {
            let kind = binary_kind(&bin.op).ok_or_else(|| {
                Error::Unsupported(format!("unsupported binary operator: {:?}", bin.op))
            })?;
            Ok(binary(
                kind,
                expr(&bin.left, allow_ident)?,
                expr(&bin.right, allow_ident)?,
            ))
        }
        syn::Expr::Unary(unary) => {
            let inner = expr(&unary.expr, allow_ident)?;
            match unary.op {
                syn::UnOp::Neg(_) => Ok(Expr::UnaryOp(UnaryOpKind::Neg, Box::new(inner))),
                syn::UnOp::Not(_) => Ok(Expr::UnaryOp(UnaryOpKind::Not, Box::new(inner))),
                _ => Err(Error::Unsupported("unsupported unary operator".to_string())),
            }
        }
        // Briev has no paren node, so parentheses are transparent here too.
        syn::Expr::Paren(paren) => expr(&paren.expr, allow_ident),
        other => Err(Error::Unsupported(format!(
            "unsupported expression (pure-integer subset): {}",
            describe(other)
        ))),
    }
}

/// Recognise `a.max(b)`, `a.min(b)` and `a.abs()`, with exact arity.
enum IntegerMethod {
    Max(Expr, Expr),
    Min(Expr, Expr),
    Abs(Expr),
}

/// Build the branch structure of a supported integer method call.
fn integer_method(call: &syn::ExprMethodCall) -> Option<IntegerMethod> {
    let receiver = expr(&call.receiver, true).ok()?;
    let argument = || expr(&call.args[0], true).ok();
    match (call.method.to_string().as_str(), call.args.len()) {
        ("max", 1) => Some(IntegerMethod::Max(receiver, argument()?)),
        ("min", 1) => Some(IntegerMethod::Min(receiver, argument()?)),
        ("abs", 0) => Some(IntegerMethod::Abs(receiver)),
        _ => None,
    }
}

/// The value of a recognised method call, as guarded `term` statements.
fn method_terms(method: IntegerMethod) -> Vec<Statement> {
    let mut out = Vec::new();
    match method {
        IntegerMethod::Max(a, b) => {
            out.push(Statement::Guarded(
                binary(BinaryOpKind::Ge, a.clone(), b.clone()),
                vec![Statement::Term(Some(a))],
            ));
            out.push(Statement::Term(Some(b)));
        }
        IntegerMethod::Min(a, b) => {
            out.push(Statement::Guarded(
                binary(BinaryOpKind::Le, a.clone(), b.clone()),
                vec![Statement::Term(Some(a))],
            ));
            out.push(Statement::Term(Some(b)));
        }
        IntegerMethod::Abs(a) => {
            out.push(Statement::Guarded(
                binary(BinaryOpKind::Ge, a.clone(), lit(0)),
                vec![Statement::Term(Some(a.clone()))],
            ));
            out.push(Statement::Term(Some(Expr::UnaryOp(
                UnaryOpKind::Neg,
                Box::new(a),
            ))));
        }
    }
    out
}

/// Whether a statement list always produces a value (and so always returns).
fn block_terminates(stmts: &[syn::Stmt]) -> bool {
    let Some(last) = stmts.last() else {
        return false;
    };
    match last {
        syn::Stmt::Expr(syn::Expr::Return(_), _) => true,
        syn::Stmt::Expr(node, None) => expr_terminates(node),
        syn::Stmt::Expr(node, Some(_)) => matches!(node, syn::Expr::Return(_)),
        _ => false,
    }
}

/// Whether an expression used as a branch always produces a value.
fn expr_terminates(node: &syn::Expr) -> bool {
    match node {
        syn::Expr::If(if_expr) => if_terminates(if_expr),
        syn::Expr::Block(block) => block_terminates(&block.block.stmts),
        _ => true,
    }
}

/// Whether an `if`/`else` always produces a value (both arms must).
fn if_terminates(node: &syn::ExprIf) -> bool {
    let then_ok = block_terminates(&node.then_branch.stmts);
    match &node.else_branch {
        Some((_, else_expr)) => then_ok && expr_terminates(else_expr),
        None => false,
    }
}

/// Negate a condition, preferring a flipped comparison operator.
///
/// A real fact (`a <= 5`) feeds the bounds prover; a negation of a comparison
/// (`!(a > 5)`) does not, so the flip is not cosmetic.
fn invert_condition(node: &syn::Expr) -> Result<Expr, Error> {
    let inner = match node {
        syn::Expr::Paren(paren) => paren.expr.as_ref(),
        other => other,
    };
    if let syn::Expr::Binary(bin) = inner {
        let flipped = match bin.op {
            syn::BinOp::Lt(_) => Some(BinaryOpKind::Ge),
            syn::BinOp::Le(_) => Some(BinaryOpKind::Gt),
            syn::BinOp::Gt(_) => Some(BinaryOpKind::Le),
            syn::BinOp::Ge(_) => Some(BinaryOpKind::Lt),
            syn::BinOp::Eq(_) => Some(BinaryOpKind::Neq),
            syn::BinOp::Ne(_) => Some(BinaryOpKind::Eq),
            _ => None,
        };
        if let Some(kind) = flipped {
            return Ok(binary(
                kind,
                expr(&bin.left, true)?,
                expr(&bin.right, true)?,
            ));
        }
    }
    Ok(Expr::UnaryOp(UnaryOpKind::Not, Box::new(expr(node, true)?)))
}

/// Build an `else` arm: a block, an `else if` chain, or a bare value.
fn branch(node: &syn::Expr) -> Result<Vec<Statement>, Error> {
    match node {
        syn::Expr::Block(node) => block(&node.block.stmts),
        syn::Expr::If(if_expr) => if_statements(if_expr),
        other => Ok(vec![Statement::Term(Some(expr(other, true)?))]),
    }
}

/// Build the statements of an `if`/`else`, mirroring the text emitter.
///
/// When both arms always produce a value, the else body is emitted *unguarded*:
/// reaching it implies the condition failed, because the then-arm terminated.
/// Otherwise the else arm is guarded by the inverted condition, which also gives
/// the prover a usable numeric fact.
fn if_statements(node: &syn::ExprIf) -> Result<Vec<Statement>, Error> {
    let cond = expr(&node.cond, true)?;
    let then_body = block(&node.then_branch.stmts)?;
    let mut out = vec![Statement::Guarded(cond, then_body)];
    let Some((_, else_expr)) = &node.else_branch else {
        return Ok(out);
    };
    let else_body = branch(else_expr)?;
    if block_terminates(&node.then_branch.stmts) && expr_terminates(else_expr) {
        out.extend(else_body);
        return Ok(out);
    }
    out.push(Statement::Guarded(invert_condition(&node.cond)?, else_body));
    Ok(out)
}

/// Build one statement, appending zero or more nodes.
fn statement(stmt: &syn::Stmt, is_last: bool, out: &mut Vec<Statement>) -> Result<(), Error> {
    match stmt {
        syn::Stmt::Local(local) => {
            let syn::Pat::Ident(pat) = &local.pat else {
                return Err(Error::Unsupported(
                    "only plain `let x = ...;` bindings are supported".to_string(),
                ));
            };
            let Some(init) = &local.init else {
                return Err(Error::Unsupported(
                    "`let` without an initializer is not supported".to_string(),
                ));
            };
            out.push(Statement::Let {
                name: pat.ident.to_string(),
                names: Vec::new(),
                ty: None,
                expr: Some(expr(&init.expr, true)?),
                modifiers: Vec::new(),
            });
            Ok(())
        }
        syn::Stmt::Expr(node, semi) => expr_statement(node, semi.is_none(), is_last, out),
        other => Err(Error::Unsupported(format!(
            "unsupported statement: {other:?}"
        ))),
    }
}

/// Build an expression statement: `return`, `if`, an assignment or a tail value.
fn expr_statement(
    node: &syn::Expr,
    no_semi: bool,
    is_last: bool,
    out: &mut Vec<Statement>,
) -> Result<(), Error> {
    match node {
        syn::Expr::Return(ret) => {
            let Some(value) = &ret.expr else {
                return Err(Error::Unsupported(
                    "bare `return;` has no value in the pure-integer subset".to_string(),
                ));
            };
            out.push(Statement::Term(Some(expr(value, true)?)));
            Ok(())
        }
        syn::Expr::If(if_expr) => {
            out.extend(if_statements(if_expr)?);
            Ok(())
        }
        syn::Expr::ForLoop(_) => Err(Error::Unsupported(
            "loops are not yet built in-process".to_string(),
        )),
        syn::Expr::Assign(assign) => {
            let syn::Expr::Path(path) = assign.left.as_ref() else {
                return Err(Error::Unsupported(
                    "assignment target must be a plain name".to_string(),
                ));
            };
            let Some(target) = path.path.get_ident() else {
                return Err(Error::Unsupported(
                    "assignment target must be a plain name".to_string(),
                ));
            };
            let name = target.to_string();
            if let syn::Expr::MethodCall(call) = assign.right.as_ref() {
                let method = integer_method(call).ok_or_else(|| {
                    Error::Unsupported(format!(
                        "unsupported expression (pure-integer subset): {}",
                        describe(assign.right.as_ref())
                    ))
                })?;
                out.extend(method_assignment(&name, method));
                return Ok(());
            }
            out.push(Statement::Assign(ident(&name), expr(&assign.right, true)?));
            Ok(())
        }
        syn::Expr::MethodCall(call) if no_semi && is_last => {
            let method = integer_method(call).ok_or_else(|| {
                Error::Unsupported(format!(
                    "unsupported expression (pure-integer subset): {}",
                    describe(node)
                ))
            })?;
            out.extend(method_terms(method));
            Ok(())
        }
        other if no_semi && is_last => {
            out.push(Statement::Term(Some(expr(other, true)?)));
            Ok(())
        }
        other => Err(Error::Unsupported(format!(
            "unsupported statement (pure-integer subset): {}",
            describe(other)
        ))),
    }
}

/// A recognised method call assigned to a name: bind once, then guard the better
/// value.
fn method_assignment(target: &str, method: IntegerMethod) -> Vec<Statement> {
    let mut out = Vec::new();
    match method {
        IntegerMethod::Max(a, b) => {
            out.push(Statement::Assign(ident(target), a.clone()));
            out.push(Statement::Guarded(
                binary(BinaryOpKind::Gt, b.clone(), a),
                vec![Statement::Assign(ident(target), b)],
            ));
        }
        IntegerMethod::Min(a, b) => {
            out.push(Statement::Assign(ident(target), a.clone()));
            out.push(Statement::Guarded(
                binary(BinaryOpKind::Lt, b.clone(), a),
                vec![Statement::Assign(ident(target), b)],
            ));
        }
        IntegerMethod::Abs(a) => {
            out.push(Statement::Assign(ident(target), a.clone()));
            out.push(Statement::Guarded(
                binary(BinaryOpKind::Lt, a.clone(), lit(0)),
                vec![Statement::Assign(
                    ident(target),
                    Expr::UnaryOp(UnaryOpKind::Neg, Box::new(a)),
                )],
            ));
        }
    }
    out
}

/// Build a statement list.
fn block(stmts: &[syn::Stmt]) -> Result<Vec<Statement>, Error> {
    let mut out = Vec::new();
    let last_index = stmts.len().saturating_sub(1);
    for (index, stmt) in stmts.iter().enumerate() {
        statement(stmt, index == last_index, &mut out)?;
    }
    Ok(out)
}

/// Read a contract attribute's expression, rejecting duplicates.
fn contract_attr(attrs: &[syn::Attribute], name: &str) -> Result<Option<syn::Expr>, Error> {
    let mut found: Option<syn::Expr> = None;
    for attr in attrs {
        if !attr.path().is_ident(name) {
            continue;
        }
        if found.is_some() {
            return Err(Error::Contract(format!(
                "more than one `#[{name}(...)]` on this function — keep one"
            )));
        }
        found = Some(
            attr.parse_args()
                .map_err(|e| Error::Parse(format!("`#[{name}(...)]`: {e}")))?,
        );
    }
    Ok(found)
}

/// The parameters and the facts they contribute.
fn signature_facts(name: &str, sig: &syn::Signature) -> Result<Vec<(String, Option<Expr>)>, Error> {
    let mut facts = Vec::new();
    for input in &sig.inputs {
        let syn::FnArg::Typed(pat_ty) = input else {
            return Err(Error::Unsupported(format!(
                "`{name}`: methods with `self` are not supported"
            )));
        };
        let width = width_of(&pat_ty.ty).ok_or_else(|| {
            Error::Unsupported(format!("`{name}`: parameter is not an integer type"))
        })?;
        let syn::Pat::Ident(pat) = pat_ty.pat.as_ref() else {
            return Err(Error::Unsupported(format!(
                "`{name}`: only plain identifier parameters are supported"
            )));
        };
        let ident = pat.ident.to_string();
        facts.push((ident.clone(), domain_fact(&ident, width)));
    }
    if let syn::ReturnType::Type(_, ty) = &sig.output {
        if width_of(ty).is_none() {
            return Err(Error::Unsupported(format!(
                "`{name}`: return type is not an integer type"
            )));
        }
    }
    Ok(facts)
}

/// Build one contracted function as a prover input.
fn definition(item_fn: &syn::ItemFn) -> Result<TopLevel, Error> {
    let name = item_fn.sig.ident.to_string();
    let facts = signature_facts(&name, &item_fn.sig)?;
    let explicit_pre = contract_attr(&item_fn.attrs, "pre")?
        .map(|node| expr(&node, true))
        .transpose()?;
    let post = contract_attr(&item_fn.attrs, "post")?
        .map(|node| expr(&node, true))
        .transpose()?;

    // Domain facts come first so the author's precondition reads as the head of
    // the conjunction, exactly as the text emitter wrote it.
    let mut parts: Vec<Expr> = facts.into_iter().filter_map(|(_, fact)| fact).collect();
    parts.extend(explicit_pre);
    let pre = conjunction(parts).unwrap_or(Expr::Bool(true));
    let post = post.unwrap_or(Expr::Bool(true));

    let body = block(&item_fn.block.stmts)?;
    Ok(TopLevel::Definition(Definition::prover_defn(
        name,
        Contract::proven(pre, post),
        body,
    )))
}

/// Build every contracted function in a parsed Rust file.
pub fn definitions(file: &syn::File) -> Result<Vec<TopLevel>, Error> {
    let mut out = Vec::new();
    for item in &file.items {
        let syn::Item::Fn(item_fn) = item else {
            continue;
        };
        let has_pre = item_fn.attrs.iter().any(|a| a.path().is_ident("pre"));
        let has_post = item_fn.attrs.iter().any(|a| a.path().is_ident("post"));
        if !has_pre && !has_post {
            continue;
        }
        out.push(definition(item_fn)?);
    }
    Ok(out)
}
