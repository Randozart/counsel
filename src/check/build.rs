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

use super::prover::ast::{BinOp, Contract, Defn, Expr, Stmt, UnOp};
use super::Error;

/// An integer literal node.
fn lit(value: i64) -> Expr {
    Expr::Int(value)
}

/// A bare identifier node.
fn ident(name: &str) -> Expr {
    Expr::Name(name.to_string())
}

/// A binary operation node.
fn binary(kind: BinOp, left: Expr, right: Expr) -> Expr {
    Expr::Binary(kind, Box::new(left), Box::new(right))
}

/// `left && right`.
fn and(left: Expr, right: Expr) -> Expr {
    binary(BinOp::And, left, right)
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
        // 2026-11-14: a `bool` is an integer in this model, exactly as its literals
        // (1/0) and its comparisons are. Accepting the *literal* while rejecting the
        // *type* was incoherent, and it blocked functions whose only non-integer part
        // was a boolean result — including every predicate in the census.
        "bool" => "i64",
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
            binary(BinOp::Ge, ident(name), lit(lo)),
            binary(BinOp::Le, ident(name), lit(hi)),
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
        "u64" | "usize" => binary(BinOp::Ge, ident(name), lit(0)),
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
        // Named rather than lumped together: an unnamed reason is unmeasurable, and
        // "an expression" was hiding 63 functions in the first vertical-slice census.
        syn::Expr::If(_) => "an `if` expression in value position".to_string(),
        syn::Expr::Block(_) => "a block expression".to_string(),
        syn::Expr::Paren(_) => "a parenthesised expression".to_string(),
        syn::Expr::Cast(_) => "a cast".to_string(),
        syn::Expr::Reference(_) => "a reference".to_string(),
        syn::Expr::Unary(_) => "a unary expression".to_string(),
        syn::Expr::Binary(_) => "a binary expression".to_string(),
        syn::Expr::Assign(_) => "an assignment in value position".to_string(),
        syn::Expr::Tuple(_) => "a tuple literal".to_string(),
        syn::Expr::Array(_) => "an array literal".to_string(),
        syn::Expr::Range(_) => "a range".to_string(),
        syn::Expr::Loop(_) => "a `loop` expression".to_string(),
        syn::Expr::While(_) => "a `while` loop in value position".to_string(),
        syn::Expr::Let(_) => "a `let` in condition position".to_string(),
        syn::Expr::Await(_) => "an `.await`".to_string(),
        syn::Expr::Unsafe(_) => "an `unsafe` block".to_string(),
        syn::Expr::Repeat(_) => "a repeated array literal".to_string(),
        syn::Expr::Macro(_) => "a macro call".to_string(),
        syn::Expr::Group(_) => "a grouped expression".to_string(),
        syn::Expr::ForLoop(_) => "a `for` loop in value position".to_string(),
        syn::Expr::Break(_) => "a `break`".to_string(),
        syn::Expr::Continue(_) => "a `continue`".to_string(),
        syn::Expr::Const(_) => "a `const` block".to_string(),
        syn::Expr::Infer(_) => "an inferred type".to_string(),
        syn::Expr::RawAddr(_) => "a raw address".to_string(),
        syn::Expr::Return(_) => "a `return` in value position".to_string(),
        syn::Expr::Async(_) => "an `async` block".to_string(),
        syn::Expr::Yield(_) => "a `yield`".to_string(),
        syn::Expr::TryBlock(_) => "a `try` block".to_string(),
        _ => "an expression outside the integer subset".to_string(),
    }
}

/// Map a Rust binary operator onto the prover's operator kind.
fn binary_kind(op: &syn::BinOp) -> Option<BinOp> {
    Some(match op {
        syn::BinOp::Add(_) => BinOp::Add,
        syn::BinOp::Sub(_) => BinOp::Sub,
        syn::BinOp::Mul(_) => BinOp::Mul,
        syn::BinOp::Div(_) => BinOp::Div,
        syn::BinOp::Rem(_) => BinOp::Rem,
        syn::BinOp::Eq(_) => BinOp::Eq,
        syn::BinOp::Ne(_) => BinOp::Ne,
        syn::BinOp::Lt(_) => BinOp::Lt,
        syn::BinOp::Le(_) => BinOp::Le,
        syn::BinOp::Gt(_) => BinOp::Gt,
        syn::BinOp::Ge(_) => BinOp::Ge,
        syn::BinOp::And(_) => BinOp::And,
        syn::BinOp::Or(_) => BinOp::Or,
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
            // 2026-11-14: a boolean literal is an integer in this model, exactly as
            // comparisons fold (`symbolic.rs`: `i64::from(left < right)`). Rejecting
            // it blocked 2,802 of brievc's functions for no semantic reason — the
            // most common single rejection reason in the census after method calls.
            syn::Lit::Bool(value) => Ok(lit(i64::from(value.value))),
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
                syn::UnOp::Neg(_) => Ok(Expr::Unary(UnOp::Neg, Box::new(inner))),
                syn::UnOp::Not(_) => Ok(Expr::Unary(UnOp::Not, Box::new(inner))),
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
fn method_terms(method: IntegerMethod) -> Vec<Stmt> {
    let mut out = Vec::new();
    match method {
        IntegerMethod::Max(a, b) => {
            out.push(Stmt::Guard(
                binary(BinOp::Ge, a.clone(), b.clone()),
                vec![Stmt::Term(Some(a))],
            ));
            out.push(Stmt::Term(Some(b)));
        }
        IntegerMethod::Min(a, b) => {
            out.push(Stmt::Guard(
                binary(BinOp::Le, a.clone(), b.clone()),
                vec![Stmt::Term(Some(a))],
            ));
            out.push(Stmt::Term(Some(b)));
        }
        IntegerMethod::Abs(a) => {
            out.push(Stmt::Guard(
                binary(BinOp::Ge, a.clone(), lit(0)),
                vec![Stmt::Term(Some(a.clone()))],
            ));
            out.push(Stmt::Term(Some(Expr::Unary(UnOp::Neg, Box::new(a)))));
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
            syn::BinOp::Lt(_) => Some(BinOp::Ge),
            syn::BinOp::Le(_) => Some(BinOp::Gt),
            syn::BinOp::Gt(_) => Some(BinOp::Le),
            syn::BinOp::Ge(_) => Some(BinOp::Lt),
            syn::BinOp::Eq(_) => Some(BinOp::Ne),
            syn::BinOp::Ne(_) => Some(BinOp::Eq),
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
    Ok(Expr::Unary(UnOp::Not, Box::new(expr(node, true)?)))
}

/// Build an `else` arm: a block, an `else if` chain, or a bare value.
fn branch(node: &syn::Expr) -> Result<Vec<Stmt>, Error> {
    match node {
        syn::Expr::Block(node) => block(&node.block.stmts),
        syn::Expr::If(if_expr) => if_statements(if_expr),
        other => Ok(vec![Stmt::Term(Some(expr(other, true)?))]),
    }
}

/// Build the statements of an `if`/`else`, mirroring the text emitter.
///
/// When both arms always produce a value, the else body is emitted *unguarded*:
/// reaching it implies the condition failed, because the then-arm terminated.
/// Otherwise the else arm is guarded by the inverted condition, which also gives
/// the prover a usable numeric fact.
fn if_statements(node: &syn::ExprIf) -> Result<Vec<Stmt>, Error> {
    let cond = expr(&node.cond, true)?;
    let then_body = block(&node.then_branch.stmts)?;
    let mut out = vec![Stmt::Guard(cond, then_body)];
    let Some((_, else_expr)) = &node.else_branch else {
        return Ok(out);
    };
    let else_body = branch(else_expr)?;
    if block_terminates(&node.then_branch.stmts) && expr_terminates(else_expr) {
        out.extend(else_body);
        return Ok(out);
    }
    out.push(Stmt::Guard(invert_condition(&node.cond)?, else_body));
    Ok(out)
}

/// The identifier a `let` binds, looking through a type ascription.
///
/// `let x: T = e;` differs from `let x = e;` only by a check rustc has already
/// performed, so the annotation is ignored rather than refused — it was 5,599 of
/// brievc's 40,193 bindings. Anything with structure (a tuple, a struct pattern,
/// `ref`) still needs a value model and stays refused.
fn binding_ident(pat: &syn::Pat) -> Option<&syn::PatIdent> {
    match pat {
        syn::Pat::Ident(ident) => Some(ident),
        syn::Pat::Type(ascribed) => binding_ident(&ascribed.pat),
        _ => None,
    }
}

/// Build one statement, appending zero or more nodes.
fn statement(stmt: &syn::Stmt, is_last: bool, out: &mut Vec<Stmt>) -> Result<(), Error> {
    match stmt {
        syn::Stmt::Local(local) => {
            let Some(pat) = binding_ident(&local.pat) else {
                return Err(Error::Unsupported(
                    "only plain `let x = ...;` bindings are supported".to_string(),
                ));
            };
            let Some(init) = &local.init else {
                return Err(Error::Unsupported(
                    "`let` without an initializer is not supported".to_string(),
                ));
            };
            out.push(Stmt::Let {
                name: pat.ident.to_string(),
                value: expr(&init.expr, true)?,
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
    out: &mut Vec<Stmt>,
) -> Result<(), Error> {
    match node {
        syn::Expr::Return(ret) => {
            let Some(value) = &ret.expr else {
                return Err(Error::Unsupported(
                    "bare `return;` has no value in the pure-integer subset".to_string(),
                ));
            };
            out.push(Stmt::Term(Some(expr(value, true)?)));
            Ok(())
        }
        syn::Expr::If(if_expr) => {
            out.extend(if_statements(if_expr)?);
            Ok(())
        }
        syn::Expr::ForLoop(for_loop) => unroll_loop(for_loop, out),
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
            out.push(Stmt::Assign(ident(&name), expr(&assign.right, true)?));
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
            out.push(Stmt::Term(Some(expr(other, true)?)));
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
fn method_assignment(target: &str, method: IntegerMethod) -> Vec<Stmt> {
    let mut out = Vec::new();
    match method {
        IntegerMethod::Max(a, b) => {
            out.push(Stmt::Assign(ident(target), a.clone()));
            out.push(Stmt::Guard(
                binary(BinOp::Gt, b.clone(), a),
                vec![Stmt::Assign(ident(target), b)],
            ));
        }
        IntegerMethod::Min(a, b) => {
            out.push(Stmt::Assign(ident(target), a.clone()));
            out.push(Stmt::Guard(
                binary(BinOp::Lt, b.clone(), a),
                vec![Stmt::Assign(ident(target), b)],
            ));
        }
        IntegerMethod::Abs(a) => {
            out.push(Stmt::Assign(ident(target), a.clone()));
            out.push(Stmt::Guard(
                binary(BinOp::Lt, a.clone(), lit(0)),
                vec![Stmt::Assign(
                    ident(target),
                    Expr::Unary(UnOp::Neg, Box::new(a)),
                )],
            ));
        }
    }
    out
}

/// Build a statement list.
fn block(stmts: &[syn::Stmt]) -> Result<Vec<Stmt>, Error> {
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
        // A receiver is one more opaque handle: `self` becomes an unconstrained value
        // with no fact, and every real use of it (field access, a method on it) is
        // still refused by the expression check. Its mere presence blocked 2,639 of
        // brievc's functions, which is an admission decision, not a modelling one.
        let pat_ty = match input {
            syn::FnArg::Receiver(_) => {
                facts.push(("self".to_string(), None));
                continue;
            }
            syn::FnArg::Typed(pat_ty) => pat_ty,
        };
        // A parameter the integer model cannot represent — a slice, a pointer, a
        // struct — is admitted as an **opaque** value: no domain fact, and any use
        // of it in the body is still refused by the expression check (indexing,
        // fields, reflection). So nothing is assumed about its contents; the
        // declaration becomes checkable when its contract is about the integers.
        let width = width_of(&pat_ty.ty);
        let syn::Pat::Ident(pat) = pat_ty.pat.as_ref() else {
            return Err(Error::Unsupported(format!(
                "`{name}`: only plain identifier parameters are supported"
            )));
        };
        let ident = pat.ident.to_string();
        facts.push((
            ident.clone(),
            width.and_then(|width| domain_fact(&ident, width)),
        ));
    }
    // The return type does not have to be an integer either. An opaque return is the
    // same admission as an opaque parameter: `term` is an unconstrained value, so a
    // postcondition that tries to say something about it simply fails to prove — it
    // can never be proved falsely. This was the single largest blocker in the census
    // (3,626 functions), and it was purely a spelling check on the signature.
    Ok(facts)
}

/// Unrolling limit, matching the text emitter.
const MAX_LOOP_UNROLL: i64 = 64;

/// An integer literal, allowing a leading negation.
fn literal_int(node: &syn::Expr) -> Option<i64> {
    match node {
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

/// Replaces the loop variable with a constant in a cloned body.
struct SubstituteLoopVar<'a> {
    name: &'a str,
    value: i64,
}

impl syn::visit_mut::VisitMut for SubstituteLoopVar<'_> {
    /// Substitute at each identifier, then keep descending.
    fn visit_expr_mut(&mut self, node: &mut syn::Expr) {
        let is_target = matches!(node, syn::Expr::Path(path) if path.path.is_ident(self.name));
        if is_target {
            *node = syn::parse_str(&self.value.to_string()).expect("loop index literal");
            return;
        }
        syn::visit_mut::visit_expr_mut(self, node);
    }
}

/// Unroll a constant-bounded `for` loop.
///
/// A variable trip count cannot be unrolled soundly, so it is refused rather
/// than approximated — an invariant is the way to verify such a loop.
fn unroll_loop(for_loop: &syn::ExprForLoop, out: &mut Vec<Stmt>) -> Result<(), Error> {
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
             needs a loop invariant (`#[invariant(...)]`). Unrolling a variable trip \
             count would be unsound, so it is refused rather than approximated"
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
        let mut body = for_loop.body.clone();
        let mut swap = SubstituteLoopVar {
            name: &name,
            value: step,
        };
        syn::visit_mut::VisitMut::visit_block_mut(&mut swap, &mut body);
        out.extend(block(&body.stmts)?);
    }
    Ok(())
}

/// A value expression as statements, desugaring a supported method call.
fn value_statements(node: &syn::Expr) -> Result<Vec<Stmt>, Error> {
    if let syn::Expr::MethodCall(call) = node {
        let method = integer_method(call).ok_or_else(|| {
            Error::Unsupported(format!(
                "unsupported expression (pure-integer subset): {}",
                describe(node)
            ))
        })?;
        return Ok(method_terms(method));
    }
    Ok(vec![Stmt::Term(Some(expr(node, true)?))])
}

/// The domain facts followed by the author's precondition.
fn precondition(item_fn: &syn::ItemFn) -> Result<Vec<Expr>, Error> {
    let name = item_fn.sig.ident.to_string();
    let facts = signature_facts(&name, &item_fn.sig)?;
    let mut parts: Vec<Expr> = facts.into_iter().filter_map(|(_, fact)| fact).collect();
    if let Some(node) = contract_attr(&item_fn.attrs, "pre")? {
        parts.push(expr(&node, true)?);
    }
    Ok(parts)
}

/// The shape of the one `for` loop an invariant applies to.
struct LoopShape {
    var: String,
    lower: Expr,
    bound: Expr,
    closed: bool,
    at: usize,
    body: Vec<syn::Stmt>,
    tail: syn::Expr,
}

/// Locate the loop an invariant applies to and the value returned after it.
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
                syn::Stmt::Expr(node, None) => Some(node),
                _ => None,
            })
            .ok_or_else(|| {
                Error::Unsupported(
                    "`#[invariant(...)]` needs the loop followed by a returned value, so \
                     the exit state can be checked against the postcondition"
                        .to_string(),
                )
            })?;
        return Ok(LoopShape {
            var: pat.ident.to_string(),
            lower: expr(start, true)?,
            bound: expr(end, true)?,
            closed: matches!(range.limits, syn::RangeLimits::Closed(_)),
            at,
            body: for_loop.body.stmts.clone(),
            tail: tail.clone(),
        });
    }
    Err(Error::Unsupported(
        "`#[invariant(...)]` needs a `for` loop over `lo..hi` to apply to".to_string(),
    ))
}

/// Every local a loop obligation must carry, from either source.
///
/// `let` bindings made before the loop supply the bounds and the entry values
/// VC1 pins; anything the body assigns may be nested inside a guard, so the scan
/// descends into `if`/blocks rather than looking one level deep.
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
    collect_assigned(&shape.body, &mut locals);
    locals
}

/// Record every assignment target, descending into guards and blocks.
fn collect_assigned(stmts: &[syn::Stmt], out: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            syn::Stmt::Expr(node, _) => collect_assigned_expr(node, out),
            syn::Stmt::Local(local) => {
                if let Some(init) = local.init.as_ref() {
                    collect_assigned_expr(&init.expr, out);
                }
            }
            _ => {}
        }
    }
}

/// Record the target of an assignment wherever it appears.
fn collect_assigned_expr(node: &syn::Expr, out: &mut Vec<String>) {
    match node {
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
        syn::Expr::Block(blocked) => collect_assigned(&blocked.block.stmts, out),
        syn::Expr::ForLoop(for_loop) => collect_assigned(&for_loop.body.stmts, out),
        syn::Expr::While(while_loop) => collect_assigned(&while_loop.body.stmts, out),
        syn::Expr::Loop(loop_expr) => collect_assigned(&loop_expr.body.stmts, out),
        syn::Expr::Binary(bin) => {
            collect_assigned_expr(&bin.left, out);
            collect_assigned_expr(&bin.right, out);
        }
        _ => {}
    }
}

/// The entry pins VC1 checks: each carried local against its initialiser, and
/// the loop variable against the lower bound.
fn entry_pins(
    item_fn: &syn::ItemFn,
    shape: &LoopShape,
    locals: &[String],
) -> Result<Vec<Expr>, Error> {
    let mut pins: Vec<Expr> = Vec::new();
    for stmt in item_fn.block.stmts.iter().take(shape.at) {
        let syn::Stmt::Local(local) = stmt else {
            continue;
        };
        let syn::Pat::Ident(pat) = &local.pat else {
            continue;
        };
        let local_name = pat.ident.to_string();
        if !locals.contains(&local_name) {
            continue;
        }
        let Some(init) = local.init.as_ref() else {
            continue;
        };
        pins.push(binary(
            BinOp::Eq,
            ident(&local_name),
            expr(&init.expr, true)?,
        ));
    }
    pins.push(binary(BinOp::Eq, ident(&shape.var), shape.lower.clone()));
    Ok(pins)
}

/// Emit the three Hoare obligations for an invariant-carrying loop.
///
/// All three are required: `_preserve` alone says the invariant survives one
/// step, not that it holds on entry or is worth anything at exit.
fn loop_contracts(item_fn: &syn::ItemFn, invariant: &syn::Expr) -> Result<Vec<Defn>, Error> {
    let name = item_fn.sig.ident.to_string();
    let shape = loop_shape(item_fn)?;
    let locals = loop_locals(item_fn, &shape);
    let pins = entry_pins(item_fn, &shape, &locals)?;
    let invariant = expr(invariant, true)?;
    let pre = precondition(item_fn)?;
    let post = contract_attr(&item_fn.attrs, "post")?
        .map(|node| expr(&node, true))
        .transpose()?
        .unwrap_or(Expr::Bool(true));
    let body = block(&shape.body)?;
    let (within, past) = if shape.closed {
        (
            binary(BinOp::Le, ident(&shape.var), shape.bound.clone()),
            binary(BinOp::Gt, ident(&shape.var), shape.bound.clone()),
        )
    } else {
        (
            binary(BinOp::Lt, ident(&shape.var), shape.bound.clone()),
            binary(BinOp::Ge, ident(&shape.var), shape.bound.clone()),
        )
    };

    // VC1: the invariant holds on entry, so the antecedent pins the locals
    // rather than assuming it.
    let mut init_parts = pre.clone();
    init_parts.extend(pins);
    let init_pre = conjunction(init_parts).unwrap_or(Expr::Bool(true));
    let init = top_level(defn(
        format!("{name}_init"),
        proven(init_pre, invariant.clone()),
        vec![Stmt::Term(Some(lit(0)))],
    ));

    // VC2: the body preserves it, with the guard and the precondition available.
    let mut preserve_parts = pre.clone();
    preserve_parts.push(invariant.clone());
    preserve_parts.push(binary(BinOp::Ge, ident(&shape.var), shape.lower.clone()));
    preserve_parts.push(within);
    let preserve_pre = conjunction(preserve_parts).unwrap_or(Expr::Bool(true));
    let mut preserve_body = body;
    preserve_body.push(Stmt::Assign(
        ident(&shape.var),
        binary(BinOp::Add, ident(&shape.var), lit(1)),
    ));
    preserve_body.push(Stmt::Term(Some(lit(0))));
    let preserve = top_level(defn(
        format!("{name}_preserve"),
        proven(preserve_pre, invariant.clone()),
        preserve_body,
    ));

    // VC3: the invariant at exit is enough to meet the contract.
    let mut exit_parts = pre;
    exit_parts.push(invariant);
    exit_parts.push(past);
    let exit_pre = conjunction(exit_parts).unwrap_or(Expr::Bool(true));
    let exit = top_level(defn(
        format!("{name}_exit"),
        proven(exit_pre, post),
        value_statements(&shape.tail)?,
    ));

    Ok(vec![init, preserve, exit])
}

/// Case values for each `#[case(...)]`, arity-checked against the parameters.
fn case_values(
    attrs: &[syn::Attribute],
    param_count: usize,
    name: &str,
) -> Result<Vec<Vec<i64>>, Error> {
    let mut out = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("case") {
            continue;
        }
        let node: syn::Expr = attr
            .parse_args()
            .map_err(|e| Error::Parse(format!("`#[case(...)]`: {e}")))?;
        let values = case_literals(&node)?;
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

/// Flatten one case: `#[case(5)]` or `#[case((2, 3))]`.
fn case_literals(node: &syn::Expr) -> Result<Vec<i64>, Error> {
    match node {
        syn::Expr::Tuple(tuple) => tuple.elems.iter().map(case_literal).collect(),
        other => Ok(vec![case_literal(other)?]),
    }
}

/// A case value must be an integer literal, optionally negated.
fn case_literal(node: &syn::Expr) -> Result<i64, Error> {
    literal_int(node).ok_or_else(|| {
        Error::Contract("`#[case(...)]` values must be integer literals".to_string())
    })
}

/// Replace parameters with their concrete case values in a built expression.
fn substitute_expr(node: &Expr, map: &[(String, i64)]) -> Expr {
    match node {
        Expr::Name(name) => match map.iter().find(|(param, _)| param == name) {
            Some((_, value)) => lit(*value),
            None => node.clone(),
        },
        Expr::Binary(kind, left, right) => Expr::Binary(
            kind.clone(),
            Box::new(substitute_expr(left, map)),
            Box::new(substitute_expr(right, map)),
        ),
        Expr::Unary(kind, inner) => {
            Expr::Unary(kind.clone(), Box::new(substitute_expr(inner, map)))
        }
        other => other.clone(),
    }
}

/// Replace parameters with their concrete case values in a built statement list.
fn substitute_stmts(stmts: &[Stmt], map: &[(String, i64)]) -> Vec<Stmt> {
    stmts
        .iter()
        .map(|stmt| substitute_stmt(stmt, map))
        .collect()
}

/// Replace parameters with their concrete case values in one statement.
fn substitute_stmt(stmt: &Stmt, map: &[(String, i64)]) -> Stmt {
    match stmt {
        Stmt::Assign(target, value) => {
            Stmt::Assign(substitute_expr(target, map), substitute_expr(value, map))
        }
        Stmt::Term(Some(value)) => Stmt::Term(Some(substitute_expr(value, map))),
        Stmt::Guard(cond, body) => {
            Stmt::Guard(substitute_expr(cond, map), substitute_stmts(body, map))
        }
        Stmt::Let { name, value } => Stmt::Let {
            name: name.clone(),
            value: substitute_expr(value, map),
        },
        other => other.clone(),
    }
}

/// The parameter names, in order.
fn parameter_names(item_fn: &syn::ItemFn) -> Vec<String> {
    item_fn
        .sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pat_ty) => match pat_ty.pat.as_ref() {
                syn::Pat::Ident(pat) => Some(pat.ident.to_string()),
                _ => None,
            },
            syn::FnArg::Receiver(_) => None,
        })
        .collect()
}

/// Instantiate a contract at each `#[case(...)]` input.
///
/// A case proves one concrete input, not the general contract, and is named
/// `<fn>__caseN` so the verdict can never be mistaken for a general proof.
fn case_definitions(item_fn: &syn::ItemFn) -> Result<Vec<Defn>, Error> {
    let name = item_fn.sig.ident.to_string();
    let params = parameter_names(item_fn);
    let cases = case_values(&item_fn.attrs, params.len(), &name)?;
    if cases.is_empty() {
        return Ok(Vec::new());
    }
    // Substitution is by name, so a body that rebinds a parameter would make the
    // case prove something other than the call it names.
    for stmt in &item_fn.block.stmts {
        let syn::Stmt::Local(local) = stmt else {
            continue;
        };
        let syn::Pat::Ident(pat) = &local.pat else {
            continue;
        };
        let rebound = pat.ident.to_string();
        if params.contains(&rebound) {
            return Err(Error::Contract(format!(
                "`#[case(...)]` on `{name}` cannot be applied: the body rebinds `{rebound}`"
            )));
        }
    }
    let pre = conjunction(precondition(item_fn)?).unwrap_or(Expr::Bool(true));
    let post = contract_attr(&item_fn.attrs, "post")?
        .map(|node| expr(&node, true))
        .transpose()?
        .unwrap_or(Expr::Bool(true));
    let body = block(&item_fn.block.stmts)?;
    let mut out = Vec::new();
    for (index, values) in cases.iter().enumerate() {
        let map: Vec<(String, i64)> = params.iter().cloned().zip(values.iter().copied()).collect();
        out.push(top_level(defn(
            format!("{name}__case{index}"),
            proven(substitute_expr(&pre, &map), substitute_expr(&post, &map)),
            substitute_stmts(&body, &map),
        )));
    }
    Ok(out)
}

// ── Call resolution ────────────────────────────────────────────────────
//
// The expression layer stays context-free, so a call is resolved here on the
// `syn` tree before any node is built: a summarised call becomes the callee's
// value expression with the caller's arguments substituted in.

/// A callee's returned value, substitutable at a call site.
///
/// Public only because [`build_summaries`] and [`inline_calls`] are; the fields
/// stay private to this module.
pub struct Summary {
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
pub fn build_summaries(items: &[syn::Item]) -> std::collections::HashMap<String, Summary> {
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
        width_of(&pat_ty.ty)?;
        params.push(ident.ident.to_string());
    }
    if let syn::ReturnType::Type(_, ty) = &sig.output {
        width_of(ty)?;
    }
    Some(params)
}

/// The single value a summary-able body returns.
fn single_value(block_node: &syn::Block) -> Option<syn::Expr> {
    if block_node
        .stmts
        .iter()
        .any(|stmt| matches!(stmt, syn::Stmt::Expr(syn::Expr::Return(_), _)))
    {
        return None;
    }
    match block_node.stmts.last()? {
        syn::Stmt::Expr(expr, None) => Some(expr.clone()),
        _ => None,
    }
}

/// Resolve summarised calls to the callee's value before building nodes.
/// The result of inlining: the rewritten function, plus what had to be assumed.
///
/// `opaque` counts unmodelled computations (constructors, associated functions)
/// that became opaque values. `unresolved` counts calls whose callee had no
/// summary — the interprocedural gap. They are reported separately because one is
/// by design and the other is a missing capability.
pub struct Inlined {
    pub item: syn::ItemFn,
    pub opaque: usize,
    pub unresolved: usize,
}

pub fn inline_calls(
    item_fn: &syn::ItemFn,
    sums: &std::collections::HashMap<String, Summary>,
) -> Result<Inlined, Error> {
    let mut inlined = item_fn.clone();
    let mut inliner = Inliner {
        sums,
        stack: Vec::new(),
        failure: None,
        opaque: 0,
        unresolved: 0,
    };
    syn::visit_mut::VisitMut::visit_item_fn_mut(&mut inliner, &mut inlined);
    match inliner.failure {
        Some(error) => Err(error),
        None => Ok(Inlined {
            item: inlined,
            opaque: inliner.opaque,
            unresolved: inliner.unresolved,
        }),
    }
}

/// Substitutes summary-able calls; leaves anything else for the builder.
struct Inliner<'a> {
    sums: &'a std::collections::HashMap<String, Summary>,
    stack: Vec<String>,
    failure: Option<Error>,
    /// How many opaque values have been minted for this function (L2a).
    opaque: usize,
    /// How many of those stood in for an unresolved *call* (L6 gap), as opposed to
    /// an unmodelled constructor.
    unresolved: usize,
}

impl Inliner<'_> {
    /// Replace `node` with a fresh opaque symbol, one per occurrence.
    ///
    /// Returns false only if the symbol could not be built, which the caller treats
    /// as a hard error rather than silently leaving an unmodelled call in place.
    fn mint_opaque(&mut self, node: &mut syn::Expr) -> bool {
        let fresh = format!("__uf{}", self.opaque);
        match syn::parse_str::<syn::Expr>(&fresh) {
            Ok(replacement) => {
                self.opaque += 1;
                *node = replacement;
                true
            }
            Err(_) => false,
        }
    }
}

impl syn::visit_mut::VisitMut for Inliner<'_> {
    /// Rewrite one expression node, recursing first so nested calls resolve.
    fn visit_expr_mut(&mut self, node: &mut syn::Expr) {
        // Lower a literal `match` into `if`/`else` before recursing, so the arms are
        // visited by the rest of this pass like any other expression.
        if let syn::Expr::Match(node_match) = node {
            if let Some(lowered) = lower_match(node_match) {
                *node = lowered;
            }
        }
        syn::visit_mut::visit_expr_mut(self, node);
        if self.failure.is_some() {
            return;
        }
        // A macro in value position produces a value we do not model (`vec![..]`,
        // `format!(..)`), so it becomes an opaque symbol on the same terms as an
        // unmodelled call: occurrence-distinct, nothing assumed, arguments dropped.
        // Statement-position macros (`println!`, `assert!`) are a different path and
        // stay refused.
        if matches!(node, syn::Expr::Macro(_)) {
            if !self.mint_opaque(node) {
                self.failure = Some(Error::Unsupported(
                    "a macro call could not be treated as opaque".to_string(),
                ));
            }
            return;
        }
        let syn::Expr::Call(call) = node else { return };
        let Some(name) = direct_name(&call.func) else {
            // L2a — an unmodelled computation (a variant constructor, `Ty::mk(x)`, an
            // associated function) becomes an OPAQUE value instead of blocking the
            // whole function.
            //
            // Each *occurrence* gets its own symbol, deliberately. Reusing one symbol
            // for identical call text would assume the callee is pure and
            // deterministic, and `let a = counter::next(); let b = counter::next();`
            // would then "prove" `a == b` — a false proof. The cost of occurrence
            // distinctness is that `Ty::V(x) == Ty::V(x)` is unprovable, which is the
            // sound direction to lose.
            //
            // The arguments are dropped rather than modelled: the value is treated as
            // an arbitrary integer, so no fact about it can be wrong.
            let fresh = format!("__uf{}", self.opaque);
            match syn::parse_str::<syn::Expr>(&fresh) {
                Ok(replacement) => {
                    self.opaque += 1;
                    *node = replacement;
                }
                Err(_) => {
                    self.failure = Some(Error::Unsupported(
                        "call through a non-name target is not supported".to_string(),
                    ));
                }
            }
            return;
        };
        let Some(summary) = self.sums.get(&name) else {
            // No summary: the callee is in another file, has a precondition, or its
            // body is outside the subset. The value is unmodelled, so it becomes an
            // opaque symbol — occurrence-distinct, arguments dropped, no fact
            // assumed. The count is reported so the interprocedural gap stays
            // visible instead of silently becoming coverage.
            if self.mint_opaque(node) {
                self.unresolved += 1;
            }
            return;
        };
        if self.stack.iter().any(|seen| seen == &name) {
            // R4: a cycle earns no summary — leave the call for the builder.
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

/// Lower `match x { 1 => a, _ => b }` into `if x == 1 { a } else { b }`.
///
/// Only the part of `match` that needs no type information is lowered: literal
/// patterns (integers and booleans) plus a final wildcard arm. Everything else —
/// enum variants, ranges, bindings, guards, or-patterns — returns `None` and the
/// function is skipped, which is the honest answer until tagged unions exist (L2b).
///
/// Two restrictions keep the rewrite sound:
///
/// 1. The **scrutinee is duplicated** once per comparison, so it must be a place or
///    a literal. Lowering `match next() { .. }` would evaluate the call several times.
/// 2. A **catch-all arm is required**. Without one the lowered chain would silently
///    become `... else { <last arm> }`, changing behaviour for unmatched values.
fn lower_match(node: &syn::ExprMatch) -> Option<syn::Expr> {
    let scrutinee = match node.expr.as_ref() {
        syn::Expr::Path(_) | syn::Expr::Lit(_) => node.expr.as_ref().clone(),
        _ => return None,
    };
    let (last, leading) = node.arms.split_last()?;
    if last.guard.is_some() {
        return None;
    }
    let mut lowered = match &last.pat {
        syn::Pat::Wild(_) => last.body.as_ref().clone(),
        _ => return None,
    };
    for arm in leading.iter().rev() {
        if arm.guard.is_some() {
            return None;
        }
        let value = literal_pattern(&arm.pat)?;
        let body = arm.body.as_ref().clone();
        let condition: syn::Expr = syn::parse_quote!(#scrutinee == #value);
        lowered = syn::parse_quote!(if #condition { #body } else { #lowered });
    }
    Some(lowered)
}

/// The literal a pattern matches, when it is an integer or boolean literal.
fn literal_pattern(pat: &syn::Pat) -> Option<syn::Expr> {
    let syn::Pat::Lit(literal) = pat else {
        return None;
    };
    match &literal.lit {
        syn::Lit::Int(_) | syn::Lit::Bool(_) => Some(syn::Expr::Lit(syn::ExprLit {
            attrs: Vec::new(),
            lit: literal.lit.clone(),
        })),
        _ => None,
    }
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

/// Build one contracted function as a prover input.
fn definition(item_fn: &syn::ItemFn) -> Result<Vec<Defn>, Error> {
    let name = item_fn.sig.ident.to_string();
    if let Some(invariant) = contract_attr(&item_fn.attrs, "invariant")? {
        return loop_contracts(item_fn, &invariant);
    }
    let pre = conjunction(precondition(item_fn)?).unwrap_or(Expr::Bool(true));
    let post = contract_attr(&item_fn.attrs, "post")?
        .map(|node| expr(&node, true))
        .transpose()?
        .unwrap_or(Expr::Bool(true));
    let body = block(&item_fn.block.stmts)?;
    Ok(vec![top_level(defn(name, proven(pre, post), body))])
}

/// Whether a function declares a contract worth proving.
/// Whether a function's **body** translates, independent of any contract.
///
/// Synthesises `[true][true]` so the builder runs its body path on a function that
/// declares no contract at all. `Ok(())` therefore means "one contract away from
/// being checkable", which is the number a census needs: it separates functions
/// blocked by a missing annotation from functions blocked by a prover gap.
pub fn probe_body(
    item_fn: &syn::ItemFn,
    sums: &std::collections::HashMap<String, Summary>,
) -> Result<(), Error> {
    let mut probe = item_fn.clone();
    probe.attrs.push(syn::parse_quote!(#[pre(true)]));
    probe.attrs.push(syn::parse_quote!(#[post(true)]));
    let Inlined { item, .. } = inline_calls(&probe, sums)?;
    definition_for(&item).map(|_| ())
}

pub fn has_contract(item_fn: &syn::ItemFn) -> bool {
    ["pre", "post", "invariant"]
        .iter()
        .any(|name| item_fn.attrs.iter().any(|attr| attr.path().is_ident(name)))
}

/// Build one function's definitions: the contract itself, plus any cases.
///
/// A loop contract expands into its obligations and takes no cases, whose
/// generated signature would not match the function the author wrote.
pub fn definition_for(item_fn: &syn::ItemFn) -> Result<Vec<Defn>, Error> {
    let is_loop = item_fn
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("invariant"));
    let mut out = definition(item_fn)?;
    if !is_loop {
        out.extend(case_definitions(item_fn)?);
    }
    Ok(out)
}

/// Build every contracted function in a parsed Rust file.
pub fn definitions(file: &syn::File) -> Result<Vec<Defn>, Error> {
    let mut out = Vec::new();
    for item in &file.items {
        let syn::Item::Fn(item_fn) = item else {
            continue;
        };
        if !has_contract(item_fn) {
            continue;
        }
        out.extend(definition_for(item_fn)?);
    }
    Ok(out)
}

/// Build one prover definition.
fn defn(name: impl Into<String>, contract: Contract, body: Vec<Stmt>) -> Defn {
    Defn {
        name: name.into(),
        contract,
        body,
    }
}

/// Identity: the builder still reads as one top-level item.
fn top_level(defn: Defn) -> Defn {
    defn
}

/// A pre/post pair the prover must discharge.
fn proven(pre: Expr, post: Expr) -> Contract {
    Contract { pre, post }
}
