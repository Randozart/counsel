// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// prover/ast.rs — the nodes Counsel hands to its own prover.
//
// Deliberately tiny. Counsel emits five expression kinds and four statement
// kinds; carrying a general language's AST here would be paying for a visitor
// and a canonicaliser it never runs.

use std::fmt;

/// A binary operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    /// The operator's source spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }

    /// Whether this operator yields a truth value rather than a number.
    pub fn is_predicate(self) -> bool {
        matches!(
            self,
            BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
                | BinOp::And
                | BinOp::Or
        )
    }

    /// The comparison that negates this one, when there is a single flip.
    ///
    /// A flipped comparison (`a <= 5`) is a usable numeric fact; `!(a > 5)` is
    /// not, so the flip matters to the bounds reasoning, not just to clarity.
    pub fn negate(self) -> Option<BinOp> {
        Some(match self {
            BinOp::Eq => BinOp::Ne,
            BinOp::Ne => BinOp::Eq,
            BinOp::Lt => BinOp::Ge,
            BinOp::Le => BinOp::Gt,
            BinOp::Gt => BinOp::Le,
            BinOp::Ge => BinOp::Lt,
            _ => return None,
        })
    }
}

/// A unary operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

/// An integer expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// A 64-bit integer literal.
    Int(i64),
    /// A truth literal, carried as 1 or 0 so one numeric model covers both.
    Bool(bool),
    /// A variable.
    Name(String),
    /// `left op right`.
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `op inner`.
    Unary(UnOp, Box<Expr>),
}

impl Expr {
    /// `left && right`.
    pub fn and(left: Expr, right: Expr) -> Expr {
        Expr::Binary(BinOp::And, Box::new(left), Box::new(right))
    }

    /// `left op right`.
    pub fn binary(op: BinOp, left: Expr, right: Expr) -> Expr {
        Expr::Binary(op, Box::new(left), Box::new(right))
    }

    /// Every variable name mentioned, in first-seen order.
    pub fn names(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_names(&mut out);
        out
    }

    /// Depth-first collection into `out`, skipping duplicates.
    fn collect_names(&self, out: &mut Vec<String>) {
        match self {
            Expr::Name(name) => {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
            Expr::Binary(_, left, right) => {
                left.collect_names(out);
                right.collect_names(out);
            }
            Expr::Unary(_, inner) => inner.collect_names(out),
            Expr::Int(_) | Expr::Bool(_) => {}
        }
    }
}

impl fmt::Display for Expr {
    /// Render for a diagnostic, matching the spelling an author wrote.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Int(value) => write!(f, "{value}"),
            Expr::Bool(value) => write!(f, "{value}"),
            Expr::Name(name) => write!(f, "{name}"),
            Expr::Binary(op, left, right) => write!(f, "({left} {} {right})", op.as_str()),
            Expr::Unary(UnOp::Neg, inner) => write!(f, "(-{inner})"),
            Expr::Unary(UnOp::Not, inner) => write!(f, "(!{inner})"),
        }
    }
}

/// A statement the prover walks.
#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    /// `let name = value;`
    Let { name: String, value: Expr },
    /// `target = value;` — the target is a name, checked by the walk.
    Assign(Expr, Expr),
    /// `when cond { body };`
    Guard(Expr, Vec<Stmt>),
    /// `term value;` — the declaration's result. `None` ends a path with no value.
    Term(Option<Expr>),
}

/// A pre/post pair.
#[derive(Clone, Debug, PartialEq)]
pub struct Contract {
    /// Assumed on entry.
    pub pre: Expr,
    /// Required of the result.
    pub post: Expr,
}

/// One declaration to prove.
#[derive(Clone, Debug, PartialEq)]
pub struct Defn {
    /// Declaration name, as reported.
    pub name: String,
    /// Its contract.
    pub contract: Contract,
    /// A straight-line body: guards fork, nothing loops or calls.
    pub body: Vec<Stmt>,
}
