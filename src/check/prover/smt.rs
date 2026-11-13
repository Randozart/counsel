// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// prover/smt.rs — encode a path to SMT-LIB and ask z3.
//
// Every value is a 64-bit bitvector, because that is what `Int` is. Comparisons
// are normalised to 1/0 with `ite`, so conjunction and disjunction are `bvand`
// and `bvor` over 0/1 and one uniform encoding covers arithmetic and predicates.
//
// Wrapping is the model's own semantics: `bvadd` wraps exactly as `Int` does, so
// a proof never silently assumes unbounded integers.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::ast::{BinOp, Expr, UnOp};

/// The solver's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmtResult {
    /// Facts and negated goal cannot hold together: the goal holds.
    Unsat,
    /// They can: either a refutation or an inconclusive model.
    Sat,
    /// No answer.
    Unknown,
}

/// Zero, as a 64-bit bitvector literal.
const ZERO: &str = "#x0000000000000000";
/// One, as a 64-bit bitvector literal.
const ONE: &str = "#x0000000000000001";

/// A 64-bit literal in SMT-LIB hexadecimal form.
fn hex(value: i64) -> String {
    format!("#x{:016x}", value as u64)
}

/// A name made safe for SMT-LIB.
fn symbol(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    format!("c_{cleaned}")
}

/// Normalise a truth-valued expression to a 0/1 bitvector.
fn truth(condition: String) -> String {
    format!("(ite {condition} {ONE} {ZERO})")
}

/// Encode one expression as a 64-bit bitvector term.
fn encode(expr: &Expr) -> String {
    match expr {
        Expr::Int(number) => hex(*number),
        Expr::Bool(flag) => if *flag { ONE } else { ZERO }.to_string(),
        Expr::Name(name) => symbol(name),
        Expr::Unary(UnOp::Neg, inner) => format!("(bvneg {})", encode(inner)),
        // Truth is 1/0, so negation is a flip of the low bit.
        Expr::Unary(UnOp::Not, inner) => format!("(bvxor {} {ONE})", encode(inner)),
        Expr::Binary(op, left, right) => {
            let left = encode(left);
            let right = encode(right);
            match op {
                BinOp::Add => format!("(bvadd {left} {right})"),
                BinOp::Sub => format!("(bvsub {left} {right})"),
                BinOp::Mul => format!("(bvmul {left} {right})"),
                BinOp::Div => format!("(bvsdiv {left} {right})"),
                BinOp::Rem => format!("(bvsrem {left} {right})"),
                BinOp::And => format!("(bvand {left} {right})"),
                BinOp::Or => format!("(bvor {left} {right})"),
                comparison => {
                    let relation = match comparison {
                        BinOp::Eq => format!("(= {left} {right})"),
                        BinOp::Ne => format!("(distinct {left} {right})"),
                        BinOp::Lt => format!("(bvslt {left} {right})"),
                        BinOp::Le => format!("(bvsle {left} {right})"),
                        BinOp::Gt => format!("(bvsgt {left} {right})"),
                        BinOp::Ge => format!("(bvsge {left} {right})"),
                        // Arithmetic was handled above.
                        _ => format!("(= {left} {right})"),
                    };
                    truth(relation)
                }
            }
        }
    }
}

/// Build the query: declare what the facts and goal mention, assert both sides.
pub fn build_query(facts: &[Expr], goal: &Expr) -> String {
    let mut names = goal.names();
    for fact in facts {
        for name in fact.names() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    let mut query = String::from("(set-logic QF_BV)\n");
    for name in &names {
        query.push_str(&format!("(declare-const {} (_ BitVec 64))\n", symbol(name)));
    }
    for fact in facts {
        query.push_str(&format!("(assert (= {} {ONE}))\n", encode(fact)));
    }
    query.push_str(&format!("(assert (not (= {} {ONE})))\n", encode(goal)));
    query.push_str("(check-sat)\n");
    query
}

/// Whether facts alone are satisfiable.
///
/// Used before reporting a refutation: a path whose facts contradict each other
/// is unreachable, and an unreachable path cannot witness a violation.
pub fn facts_satisfiable(facts: &[Expr]) -> Option<bool> {
    let mut query = String::from("(set-logic QF_BV)\n");
    let mut names: Vec<String> = Vec::new();
    for fact in facts {
        for name in fact.names() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    for name in &names {
        query.push_str(&format!("(declare-const {} (_ BitVec 64))\n", symbol(name)));
    }
    for fact in facts {
        query.push_str(&format!("(assert (= {} {ONE}))\n", encode(fact)));
    }
    query.push_str("(check-sat)\n");
    match run(&query) {
        SmtResult::Sat => Some(true),
        SmtResult::Unsat => Some(false),
        SmtResult::Unknown => None,
    }
}

/// Ask the solver whether `facts && !goal` is satisfiable.
pub fn prove(facts: &[Expr], goal: &Expr) -> SmtResult {
    run(&build_query(facts, goal))
}

/// Run one query through z3.
fn run(query: &str) -> SmtResult {
    let Some(binary) = z3_path() else {
        return SmtResult::Unknown;
    };
    let child = Command::new(binary)
        .arg("-in")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return SmtResult::Unknown;
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(query.as_bytes()).is_err() {
            return SmtResult::Unknown;
        }
    }
    let Ok(output) = child.wait_with_output() else {
        return SmtResult::Unknown;
    };
    match String::from_utf8_lossy(&output.stdout).lines().next() {
        Some("unsat") => SmtResult::Unsat,
        Some("sat") => SmtResult::Sat,
        _ => SmtResult::Unknown,
    }
}

/// The solver binary: `$Z3`, the manager-installed copy, then `PATH`.
fn z3_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("Z3") {
        return Some(PathBuf::from(explicit));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let managed = PathBuf::from(home).join(".local/share/briev-compiler/bin/z3");
        if managed.is_file() {
            return Some(managed);
        }
    }
    Some(PathBuf::from("z3"))
}

/// Whether a solver is available at all.
pub fn available() -> bool {
    let Some(binary) = z3_path() else {
        return false;
    };
    Command::new(binary)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arithmetic and comparisons encode to the operations that wrap.
    #[test]
    fn encodes_arithmetic_as_bitvector_operations() {
        let goal = Expr::binary(BinOp::Ge, Expr::Name("x".into()), Expr::Int(0));
        assert_eq!(
            encode(&goal),
            "(ite (bvsge c_x #x0000000000000000) #x0000000000000001 #x0000000000000000)"
        );
    }

    /// The query declares each name once and asserts both sides.
    #[test]
    fn query_declares_names_and_negates_the_goal() {
        let facts = vec![Expr::binary(
            BinOp::Eq,
            Expr::Name("a".into()),
            Expr::Name("b".into()),
        )];
        let goal = Expr::binary(BinOp::Eq, Expr::Name("a".into()), Expr::Name("b".into()));
        let query = build_query(&facts, &goal);
        assert_eq!(query.matches("(declare-const c_a ").count(), 1);
        assert_eq!(query.matches("(declare-const c_b ").count(), 1);
        assert!(query.contains("(assert (not (= (ite (= c_a c_b)"));
    }

    /// A relational goal is decided by the solver when one is installed.
    #[test]
    fn decides_a_relational_goal() {
        if !available() {
            return;
        }
        let facts = vec![Expr::binary(
            BinOp::Eq,
            Expr::Name("a".into()),
            Expr::Name("b".into()),
        )];
        let goal = Expr::binary(
            BinOp::Eq,
            Expr::binary(BinOp::Add, Expr::Name("a".into()), Expr::Int(1)),
            Expr::binary(BinOp::Add, Expr::Name("b".into()), Expr::Int(1)),
        );
        assert_eq!(prove(&facts, &goal), SmtResult::Unsat);
    }
}
