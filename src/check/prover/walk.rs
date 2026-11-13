// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// prover/walk.rs — fork on guards, then decide each terminating path.
//
// The body is straight-line: guards fork and nothing loops or calls, because
// Counsel resolves calls on the `syn` tree and turns loops into separate
// obligations before a definition reaches here.

use super::ast::{BinOp, Contract, Defn, Expr, Stmt, UnOp};
use super::smt::{self, SmtResult};
use super::symbolic::{bounds_of, decide_by_bounds, eval, State, Value};

/// One declaration's verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every terminating path satisfies the postcondition.
    Holds,
    /// A reachable path refutes it.
    Violated(String),
    /// No conclusion — advisory, never a failure.
    Unproven(String),
}

/// The result of verifying a batch.
#[derive(Debug, Default)]
pub struct Report {
    /// Declarations proved.
    pub proven: Vec<String>,
    /// Declarations the prover could not decide, with the reason.
    pub unproven: Vec<String>,
    /// Declarations with a reachable counterexample.
    pub violated: Vec<String>,
}

/// Verify every declaration.
pub fn verify(defns: &[Defn]) -> Report {
    let mut report = Report::default();
    for defn in defns {
        match verify_one(defn) {
            Verdict::Holds => report.proven.push(defn.name.clone()),
            Verdict::Unproven(why) => report.unproven.push(format!("{}: {why}", defn.name)),
            Verdict::Violated(why) => report.violated.push(format!("{}: {why}", defn.name)),
        }
    }
    report
}

/// Verify one declaration.
pub fn verify_one(defn: &Defn) -> Verdict {
    // The precondition is a fact on every path, recorded in the entry state.
    let mut initial = State::default();
    initial.record(&defn.contract.pre);
    pin(&mut initial, &defn.contract.pre);
    let mut terminated = Vec::new();
    walk(&defn.body, vec![initial], &mut terminated);

    if terminated.is_empty() {
        return Verdict::Unproven("body has no terminating value path".to_string());
    }

    let total = terminated.len();
    let mut undecided = None;
    for (index, state) in terminated.iter().enumerate() {
        match judge(&defn.contract, state) {
            Verdict::Holds => {}
            Verdict::Violated(why) => return Verdict::Violated(why),
            Verdict::Unproven(why) => {
                if undecided.is_none() {
                    undecided = Some(format!(
                        "could not prove the postcondition on path {} of {total} (goal: {}) — {why}",
                        index + 1,
                        defn.contract.post
                    ));
                }
            }
        }
    }
    match undecided {
        Some(why) => Verdict::Unproven(why),
        None => Verdict::Holds,
    }
}

/// Seed the constants a precondition fixes.
///
/// `acc == 0` is an equality, not a range: binding it lets an entry obligation
/// (`_init`) be decided outright instead of degrading to advisory. Without this
/// the facts constrain the solver but leave the goal's own names symbolic.
fn pin(state: &mut State, fact: &Expr) {
    match fact {
        Expr::Binary(BinOp::And, left, right) => {
            pin(state, left);
            pin(state, right);
        }
        Expr::Binary(BinOp::Eq, left, right) => {
            let pair = match (left.as_ref(), right.as_ref()) {
                (Expr::Name(name), Expr::Int(value)) => Some((name, *value)),
                (Expr::Int(value), Expr::Name(name)) => Some((name, *value)),
                _ => None,
            };
            if let Some((name, value)) = pair {
                state.assignments.insert(name.clone(), Value::Const(value));
            }
        }
        _ => {}
    }
}

/// Walk a block, returning the states that fall through it.
///
/// A `term` ends a path and lands in `terminated`; a guard walks its body and
/// lets the survivors continue after the guard, which is what `when` means.
fn walk(body: &[Stmt], initial: Vec<State>, terminated: &mut Vec<State>) -> Vec<State> {
    let mut states = initial;
    for stmt in body {
        let mut next = Vec::new();
        for state in states.drain(..) {
            match stmt {
                Stmt::Let { name, value } => {
                    let mut bound = state;
                    bound.bind(name, value);
                    next.push(bound);
                }
                Stmt::Assign(target, value) => {
                    let Expr::Name(name) = target else {
                        continue;
                    };
                    let mut bound = state;
                    bound.bind(name, value);
                    next.push(bound);
                }
                // A path that ends without a value has nothing to prove.
                Stmt::Term(None) => {}
                Stmt::Term(Some(value)) => terminated.push(bind_output(state, value)),
                Stmt::Guard(cond, inner) => {
                    let mut taken = state.clone();
                    taken.record(cond);
                    next.extend(walk(inner, vec![taken], terminated));
                    let mut skipped = state;
                    skipped.record(&negate(cond));
                    next.push(skipped);
                }
            }
        }
        states = next;
    }
    states
}

/// Bind `term` to the value a path returns.
fn bind_output(mut state: State, value: &Expr) -> State {
    let evaluated = eval(value, &state);
    state.assignments.insert("term".to_string(), evaluated);
    state
}

/// The condition that holds when `cond` does not.
///
/// A flipped comparison (`a <= 5`) is a numeric fact the bounds reasoning can
/// use; `!(a > 5)` is not, so the flip is not cosmetic.
fn negate(cond: &Expr) -> Expr {
    if let Expr::Binary(op, left, right) = cond {
        if let Some(flipped) = op.negate() {
            return Expr::Binary(flipped, left.clone(), right.clone());
        }
    }
    Expr::Unary(UnOp::Not, Box::new(cond.clone()))
}

/// Decide one terminating path.
fn judge(contract: &Contract, state: &State) -> Verdict {
    match eval(&contract.post, state) {
        // A constant truth value is a direct answer — but a contradiction in the
        // facts means the path is unreachable, and an unreachable path cannot
        // witness a violation.
        Value::Const(value) => {
            if value != 0 {
                return Verdict::Holds;
            }
            if unreachable(state) {
                return Verdict::Holds;
            }
            Verdict::Violated("a reachable path contradicts the contract".to_string())
        }
        Value::Sym(goal) => {
            if decide_by_bounds(&goal, &bounds_of(&state.facts)) == Some(true) {
                return Verdict::Holds;
            }
            decide_with_solver(&goal, state)
        }
    }
}

/// Escalate a symbolic goal to the solver.
fn decide_with_solver(goal: &Expr, state: &State) -> Verdict {
    match smt::prove(&state.facts, goal) {
        SmtResult::Unsat => Verdict::Holds,
        // A model refutes only when nothing in the goal is still free; otherwise
        // it is one input among many and stays advisory.
        SmtResult::Sat if goal.names().is_empty() => {
            Verdict::Violated("a path contradicts the contract".to_string())
        }
        SmtResult::Sat => {
            Verdict::Unproven("the solver found only a parameter-dependent model".to_string())
        }
        SmtResult::Unknown => Verdict::Unproven("the solver could not decide".to_string()),
    }
}

/// Whether the path's facts contradict each other.
fn unreachable(state: &State) -> bool {
    // A fact that evaluated to a constant false is enough on its own.
    if state
        .facts
        .iter()
        .any(|fact| matches!(eval(fact, &State::default()), Value::Const(0)))
    {
        return true;
    }
    smt::facts_satisfiable(&state.facts) == Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build `defn name [pre][post] { body }`.
    fn defn(name: &str, pre: Expr, post: Expr, body: Vec<Stmt>) -> Defn {
        Defn {
            name: name.to_string(),
            contract: Contract { pre, post },
            body,
        }
    }

    fn name(text: &str) -> Expr {
        Expr::Name(text.to_string())
    }

    fn compare(op: BinOp, left: Expr, right: Expr) -> Expr {
        Expr::binary(op, left, right)
    }

    /// `term a;` with `[a >= 0][term >= 0]`.
    #[test]
    fn a_direct_value_is_proved() {
        let defn = defn(
            "f",
            compare(BinOp::Ge, name("a"), Expr::Int(0)),
            compare(BinOp::Ge, name("term"), Expr::Int(0)),
            vec![Stmt::Term(Some(name("a")))],
        );
        assert_eq!(verify_one(&defn), Verdict::Holds);
    }

    /// A constant body that contradicts the postcondition is refuted.
    #[test]
    fn a_false_constant_is_violated() {
        let defn = defn(
            "lies",
            Expr::Bool(true),
            compare(BinOp::Eq, name("term"), Expr::Int(0)),
            vec![Stmt::Term(Some(Expr::Int(5)))],
        );
        assert!(matches!(verify_one(&defn), Verdict::Violated(_)));
    }

    /// Both arms of a guard are covered, with the negation as a usable fact.
    #[test]
    fn both_guard_arms_are_proved() {
        let body = vec![
            Stmt::Guard(
                compare(BinOp::Lt, name("a"), Expr::Int(0)),
                vec![Stmt::Term(Some(Expr::Unary(
                    UnOp::Neg,
                    Box::new(name("a")),
                )))],
            ),
            Stmt::Term(Some(name("a"))),
        ];
        // Bounded away from i64::MIN: `-a` wraps there and stays negative, so
        // the unbounded claim is false and must not be proved. The old engine
        // reasoned about negation intervals without that edge and said Holds.
        let defn = defn(
            "abs_like",
            compare(BinOp::Ge, name("a"), Expr::Int(-1000)),
            compare(BinOp::Ge, name("term"), Expr::Int(0)),
            body.clone(),
        );
        assert_eq!(verify_one(&defn), Verdict::Holds);
    }

    /// The same body, unbounded, is not provable — and is not claimed.
    #[test]
    fn unbounded_negation_is_not_proved() {
        let body = vec![
            Stmt::Guard(
                compare(BinOp::Lt, name("a"), Expr::Int(0)),
                vec![Stmt::Term(Some(Expr::Unary(
                    UnOp::Neg,
                    Box::new(name("a")),
                )))],
            ),
            Stmt::Term(Some(name("a"))),
        ];
        let defn = defn(
            "abs_unbounded",
            Expr::Bool(true),
            compare(BinOp::Ge, name("term"), Expr::Int(0)),
            body,
        );
        assert!(
            matches!(verify_one(&defn), Verdict::Unproven(_)),
            "i64::MIN makes -a negative; this must stay unproven"
        );
    }

    /// A parameter-dependent contradiction stays advisory, never a refutation.
    #[test]
    fn parameter_dependent_contradiction_is_advisory() {
        let body = vec![
            Stmt::Let {
                name: "acc".into(),
                value: Expr::Int(0),
            },
            Stmt::Assign(
                name("acc"),
                Expr::binary(BinOp::Add, name("acc"), name("a")),
            ),
            Stmt::Term(Some(name("acc"))),
        ];
        let defn = defn(
            "maybe",
            compare(BinOp::Ge, name("a"), Expr::Int(0)),
            compare(BinOp::Eq, name("term"), Expr::Int(0)),
            body,
        );
        assert!(matches!(verify_one(&defn), Verdict::Unproven(_)));
    }

    /// A body with no terminating value has nothing to prove.
    #[test]
    fn no_terminating_path_is_unproven() {
        let defn = defn("empty", Expr::Bool(true), Expr::Bool(true), Vec::new());
        assert!(matches!(verify_one(&defn), Verdict::Unproven(_)));
    }
}
