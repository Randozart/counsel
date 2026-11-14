// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// prover/symbolic.rs — values, eager substitution, and interval bounds.
//
// Substitution is EAGER: binding a name evaluates its value against the state at
// that moment. That was a deliberate choice over the lazy substitution the old
// engine used, where `acc = acc + 1` could expand its own reference twice
// (briev-lang 95dbe54c). Here the lookup happens before the rebinding, so a
// self-reference can only ever see the previous value — the defect cannot be
// expressed.

use std::collections::HashMap;

use super::ast::{BinOp, Expr, UnOp};

/// What the prover tracks for a variable.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A known number.
    Const(i64),
    /// A symbolic expression over names that are still unknown.
    Sym(Expr),
}

impl Value {
    /// The value as an expression, for encoding or reporting.
    pub fn into_expr(self) -> Expr {
        match self {
            Value::Const(number) => Expr::Int(number),
            Value::Sym(expr) => expr,
        }
    }
}

/// The state of one path.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// Variable bindings, already substituted.
    pub assignments: HashMap<String, Value>,
    /// Facts holding on this path, already substituted at the point they were
    /// recorded. Keeping them in the entry state is not an accident: rewriting
    /// them with the final state once produced a false proof (briev-lang
    /// ed2fc83d).
    pub facts: Vec<Expr>,
}

impl State {
    /// Bind `name` to `value`, evaluated against the state *before* the rebind.
    pub fn bind(&mut self, name: &str, value: &Expr) {
        let evaluated = eval(value, self);
        self.assignments.insert(name.to_string(), evaluated);
    }

    /// Record a fact, substituted against the current state.
    pub fn record(&mut self, fact: &Expr) {
        let evaluated = eval(fact, self).into_expr();
        self.facts.push(evaluated);
    }
}

/// Evaluate as far as constants allow.
///
/// Arithmetic wraps, matching the 64-bit `Int` model: a proof must not assume
/// the arithmetic is unbounded.
pub fn eval(expr: &Expr, state: &State) -> Value {
    match expr {
        Expr::Int(number) => Value::Const(*number),
        Expr::Bool(flag) => Value::Const(i64::from(*flag)),
        Expr::Name(name) => state
            .assignments
            .get(name)
            .cloned()
            .unwrap_or_else(|| Value::Sym(Expr::Name(name.clone()))),
        Expr::Unary(op, inner) => match eval(inner, state) {
            Value::Const(number) => Value::Const(fold_unary(*op, number)),
            Value::Sym(inner) => Value::Sym(Expr::Unary(*op, Box::new(inner))),
        },
        Expr::Binary(op, left, right) => {
            let left = eval(left, state);
            let right = eval(right, state);
            match (&left, &right) {
                (Value::Const(a), Value::Const(b)) => match fold_binary(*op, *a, *b) {
                    Some(value) => Value::Const(value),
                    // Division by zero stays symbolic: it is not a number we can
                    // fold, and the solver is the right place to reason about it.
                    None => Value::Sym(Expr::binary(*op, Expr::Int(*a), Expr::Int(*b))),
                },
                // A conjunction with a false operand is false whatever the other
                // side is, and a disjunction with a true operand is true. Without
                // this, `0 && (i <= n)` stays symbolic and a refuted entry
                // obligation degrades to advisory.
                _ if short_circuits(*op, &left, &right) => {
                    Value::Const(i64::from(*op == BinOp::Or))
                }
                _ => Value::Sym(Expr::binary(*op, left.into_expr(), right.into_expr())),
            }
        }
    }
}

/// Whether a boolean operator is already decided by one operand.
///
/// `false && x` is false and `true || x` is true, so the unknown operand never
/// matters.
fn short_circuits(op: BinOp, left: &Value, right: &Value) -> bool {
    let falsy = |value: &Value| matches!(value, Value::Const(0));
    let truthy = |value: &Value| matches!(value, Value::Const(number) if *number != 0);
    match op {
        BinOp::And => falsy(left) || falsy(right),
        BinOp::Or => truthy(left) || truthy(right),
        _ => false,
    }
}

/// Apply a unary operator to a constant.
fn fold_unary(op: UnOp, value: i64) -> i64 {
    match op {
        UnOp::Neg => value.wrapping_neg(),
        // Truth is 1/0, so negation of zero is one.
        UnOp::Not => i64::from(value == 0),
    }
}

/// Apply a binary operator to constants, when it yields one.
fn fold_binary(op: BinOp, left: i64, right: i64) -> Option<i64> {
    Some(match op {
        BinOp::Add => left.wrapping_add(right),
        BinOp::Sub => left.wrapping_sub(right),
        BinOp::Mul => left.wrapping_mul(right),
        BinOp::Div => {
            if right == 0 {
                return None;
            }
            left.wrapping_div(right)
        }
        BinOp::Rem => {
            if right == 0 {
                return None;
            }
            left.wrapping_rem(right)
        }
        BinOp::Eq => i64::from(left == right),
        BinOp::Ne => i64::from(left != right),
        BinOp::Lt => i64::from(left < right),
        BinOp::Le => i64::from(left <= right),
        BinOp::Gt => i64::from(left > right),
        BinOp::Ge => i64::from(left >= right),
        // Operands arrive as 0/1 already.
        BinOp::And => i64::from(left != 0 && right != 0),
        BinOp::Or => i64::from(left != 0 || right != 0),
    })
}

/// An inclusive integer interval, as far as the facts determine it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Bounds {
    /// Lowest permitted value.
    pub low: Option<i64>,
    /// Highest permitted value.
    pub high: Option<i64>,
}

/// Derive an interval per variable from a path's facts.
pub fn bounds_of(facts: &[Expr]) -> HashMap<String, Bounds> {
    let mut bounds: HashMap<String, Bounds> = HashMap::new();
    for fact in facts {
        tighten_fact(fact, &mut bounds);
    }
    bounds
}

/// Apply one fact, descending through conjunctions.
fn tighten_fact(fact: &Expr, bounds: &mut HashMap<String, Bounds>) {
    match fact {
        Expr::Binary(BinOp::And, left, right) => {
            tighten_fact(left, bounds);
            tighten_fact(right, bounds);
        }
        Expr::Binary(op, left, right) => {
            // Accept `name <op> constant` and its mirror image.
            if let (Expr::Name(name), Expr::Int(value)) = (left.as_ref(), right.as_ref()) {
                tighten(name, *op, *value, bounds);
            } else if let (Expr::Int(value), Expr::Name(name)) = (left.as_ref(), right.as_ref()) {
                if let Some(flipped) = mirror(*op) {
                    tighten(name, flipped, *value, bounds);
                }
            }
        }
        Expr::Unary(UnOp::Not, inner) => {
            // `!(a > 5)` is `a <= 5`; treat it as the comparison it means.
            if let Expr::Binary(op, left, right) = inner.as_ref() {
                if let Some(negated) = op.negate() {
                    let negated_fact = Expr::binary(negated, *left.clone(), *right.clone());
                    tighten_fact(&negated_fact, bounds);
                }
            }
        }
        _ => {}
    }
}

/// The comparison seen from the other side: `5 < a` is `a > 5`.
fn mirror(op: BinOp) -> Option<BinOp> {
    Some(match op {
        BinOp::Lt => BinOp::Gt,
        BinOp::Le => BinOp::Ge,
        BinOp::Gt => BinOp::Lt,
        BinOp::Ge => BinOp::Le,
        BinOp::Eq => BinOp::Eq,
        BinOp::Ne => BinOp::Ne,
        _ => return None,
    })
}

/// Record one bound for `name`.
fn tighten(name: &str, op: BinOp, value: i64, bounds: &mut HashMap<String, Bounds>) {
    let entry = bounds.entry(name.to_string()).or_default();
    match op {
        BinOp::Ge => entry.low = Some(entry.low.map_or(value, |old| old.max(value))),
        BinOp::Gt => entry.low = Some(entry.low.map_or(value + 1, |old| old.max(value + 1))),
        BinOp::Le => entry.high = Some(entry.high.map_or(value, |old| old.min(value))),
        BinOp::Lt => entry.high = Some(entry.high.map_or(value - 1, |old| old.min(value - 1))),
        BinOp::Eq => {
            entry.low = Some(entry.low.map_or(value, |old| old.max(value)));
            entry.high = Some(entry.high.map_or(value, |old| old.min(value)));
        }
        _ => {}
    }
}

/// Decide an interval comparison without the solver.
/// Bounds of an arbitrary expression, given the bounds of its variables.
///
/// Every rule is conservative in one direction only: when a bound cannot be computed
/// exactly it is **dropped**, never approximated in the unhelpful direction. A wrong
/// bound here would be a false proof, which is the one failure this prover must not
/// have. Overflow, division by a non-positive constant, and unknown signs all resolve
/// to `None`.
pub fn expr_bounds(expr: &Expr, bounds: &HashMap<String, Bounds>) -> Bounds {
    match expr {
        Expr::Int(value) => Bounds {
            low: Some(*value),
            high: Some(*value),
        },
        Expr::Bool(value) => {
            let truth = i64::from(*value);
            Bounds {
                low: Some(truth),
                high: Some(truth),
            }
        }
        Expr::Name(name) => bounds.get(name).copied().unwrap_or_default(),
        Expr::Unary(UnOp::Neg, inner) => {
            let inner = expr_bounds(inner, bounds);
            // `-a` is only trustworthy when the source range excludes `i64::MIN`:
            // negating the minimum wraps back to a negative value, so an unbounded
            // below operand must yield no bound at all. A known lower bound above
            // `i64::MIN` is exactly the proof that it is safe.
            match inner.low {
                Some(low) if low > i64::MIN => Bounds {
                    low: inner.high.and_then(i64::checked_neg),
                    high: Some(-low),
                },
                _ => Bounds::default(),
            }
        }
        // `!x` is `x == 0` as an integer, so it is always a truth value.
        Expr::Unary(UnOp::Not, _) => Bounds {
            low: Some(0),
            high: Some(1),
        },
        Expr::Binary(op, left, right) => {
            let left = expr_bounds(left, bounds);
            let right = expr_bounds(right, bounds);
            operator_bounds(*op, left, right)
        }
    }
}

/// Interval arithmetic for one operator, or empty bounds when it cannot be trusted.
fn operator_bounds(op: BinOp, left: Bounds, right: Bounds) -> Bounds {
    match op {
        BinOp::Add => Bounds {
            low: addition(left.low, right.low),
            high: addition(left.high, right.high),
        },
        BinOp::Sub => Bounds {
            low: left.low.zip(right.high).and_then(|(a, b)| a.checked_sub(b)),
            high: left.high.zip(right.low).and_then(|(a, b)| a.checked_sub(b)),
        },
        BinOp::Mul => product_bounds(left, right),
        BinOp::Div => quotient_bounds(left, right),
        BinOp::Rem => remainder_bounds(left, right),
        // Every comparison and logical operator yields a truth value.
        _ => Bounds {
            low: Some(0),
            high: Some(1),
        },
    }
}

/// Sum of two optional bounds, dropped on overflow.
fn addition(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    left.zip(right).and_then(|(a, b)| a.checked_add(b))
}

/// Product bounds, with a sign rule so a non-negative product is known even when one
/// operand's upper end is unbounded (`[0, ∞) * 16` is still `[0, ∞)`).
fn product_bounds(left: Bounds, right: Bounds) -> Bounds {
    if let (Some(low_left), Some(low_right)) = (left.low, right.low) {
        if low_left >= 0 && low_right >= 0 {
            return Bounds {
                low: low_left.checked_mul(low_right),
                high: None,
            };
        }
    }
    let (Some(a), Some(b), Some(c), Some(d)) = (left.low, left.high, right.low, right.high) else {
        return Bounds::default();
    };
    let mut low = i64::MAX;
    let mut high = i64::MIN;
    for (x, y) in [(a, c), (a, d), (b, c), (b, d)] {
        // Checked, not wrapping: a wrapped bound would be a wrong bound, and a wrong
        // bound is a false proof.
        let Some(product) = x.checked_mul(y) else {
            return Bounds::default();
        };
        low = low.min(product);
        high = high.max(product);
    }
    Bounds {
        low: Some(low),
        high: Some(high),
    }
}

/// Quotient bounds for a positive constant divisor and a non-negative numerator.
///
/// Truncation toward zero is exact in that quadrant, which is the only case handled;
/// everything else is dropped rather than guessed.
fn quotient_bounds(left: Bounds, right: Bounds) -> Bounds {
    let (Some(divisor), Some(low)) = (right.low, left.low) else {
        return Bounds::default();
    };
    if divisor <= 0 || right.high != right.low || low < 0 {
        return Bounds::default();
    }
    Bounds {
        low: Some(low / divisor),
        high: left.high.map(|high| high / divisor),
    }
}

/// Remainder bounds: for a positive constant divisor and a non-negative numerator the
/// result lies in `[0, divisor - 1]` whatever the numerator is.
fn remainder_bounds(left: Bounds, right: Bounds) -> Bounds {
    let (Some(divisor), Some(low)) = (right.low, left.low) else {
        return Bounds::default();
    };
    if divisor <= 0 || right.high != right.low || low < 0 {
        return Bounds::default();
    }
    Bounds {
        low: Some(0),
        high: Some(divisor - 1),
    }
}

/// Decide a comparison from operand intervals, or `None` when they overlap.
pub fn decide_by_bounds(goal: &Expr, bounds: &HashMap<String, Bounds>) -> Option<bool> {
    let Expr::Binary(op, left, right) = goal else {
        return None;
    };
    let left = expr_bounds(left, bounds);
    let right = expr_bounds(right, bounds);
    match op {
        BinOp::Ge => {
            if left.low? >= right.high? {
                Some(true)
            } else if left.high? < right.low? {
                Some(false)
            } else {
                None
            }
        }
        BinOp::Gt => {
            if left.low? > right.high? {
                Some(true)
            } else if left.high? <= right.low? {
                Some(false)
            } else {
                None
            }
        }
        BinOp::Le => {
            if left.high? <= right.low? {
                Some(true)
            } else if left.low? > right.high? {
                Some(false)
            } else {
                None
            }
        }
        BinOp::Lt => {
            if left.high? < right.low? {
                Some(true)
            } else if left.low? >= right.high? {
                Some(false)
            } else {
                None
            }
        }
        BinOp::Eq => {
            // Both sides must be *exactly* known before equality can be decided.
            // Comparing the `Option`s directly would read `None == None` as "both endpoints
            // equal" and prove `a * a == a * 2` for unbounded `a` — a false proof, and
            // one the valve suite caught.
            if let (Some(low), Some(high), Some(other_low), Some(other_high)) =
                (left.low, left.high, right.low, right.high)
            {
                if low == high && other_low == other_high {
                    return Some(low == other_low);
                }
            }
            if left.high? < right.low? || right.high? < left.low? {
                Some(false)
            } else {
                None
            }
        }
        BinOp::Ne => {
            if let (Some(low), Some(high), Some(other_low), Some(other_high)) =
                (left.low, left.high, right.low, right.high)
            {
                if low == high && other_low == other_high {
                    return Some(low != other_low);
                }
            }
            if left.high? < right.low? || right.high? < left.low? {
                Some(true)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Binding evaluates against the state before the rebind, so a
    /// self-referential assignment sees the previous value exactly once.
    #[test]
    fn self_reference_expands_once() {
        let mut state = State::default();
        state.bind("acc", &Expr::Int(0));
        state.bind(
            "acc",
            &Expr::binary(BinOp::Add, Expr::Name("acc".into()), Expr::Int(1)),
        );
        assert_eq!(state.assignments["acc"], Value::Const(1));
    }

    /// A symbolic rebind keeps the parameter and adds once.
    #[test]
    fn symbolic_self_reference_adds_once() {
        let mut state = State::default();
        state.bind("acc", &Expr::Name("n".into()));
        state.bind(
            "acc",
            &Expr::binary(BinOp::Add, Expr::Name("acc".into()), Expr::Int(1)),
        );
        assert_eq!(
            state.assignments["acc"],
            Value::Sym(Expr::binary(
                BinOp::Add,
                Expr::Name("n".into()),
                Expr::Int(1)
            ))
        );
    }

    /// Division by zero is not folded away.
    #[test]
    fn division_by_zero_stays_symbolic() {
        let state = State::default();
        let value = eval(
            &Expr::binary(BinOp::Div, Expr::Int(1), Expr::Int(0)),
            &state,
        );
        assert!(matches!(value, Value::Sym(_)), "{value:?}");
    }

    /// An interval is carried through division and multiplication, so a bound on a
    /// compound expression is decidable — the `align16` shape.
    #[test]
    fn bounds_flow_through_division_and_multiplication() {
        let mut bounds = HashMap::new();
        bounds.insert(
            "n".to_string(),
            Bounds {
                low: Some(0),
                high: None,
            },
        );
        let aligned = Expr::binary(
            BinOp::Mul,
            Expr::binary(
                BinOp::Div,
                Expr::binary(BinOp::Add, Expr::Name("n".into()), Expr::Int(15)),
                Expr::Int(16),
            ),
            Expr::Int(16),
        );
        let goal = Expr::binary(BinOp::Ge, aligned, Expr::Int(0));
        assert_eq!(decide_by_bounds(&goal, &bounds), Some(true));
    }

    /// A divisor with no known sign yields no bounds rather than a guessed one.
    #[test]
    fn division_by_an_unknown_divisor_yields_no_bounds() {
        let mut bounds = HashMap::new();
        bounds.insert(
            "a".to_string(),
            Bounds {
                low: Some(0),
                high: None,
            },
        );
        let quotient = Expr::binary(BinOp::Div, Expr::Name("a".into()), Expr::Name("d".into()));
        let found = expr_bounds(&quotient, &bounds);
        assert!(found.low.is_none() && found.high.is_none(), "{found:?}");
    }

    /// Negating an operand that may be `i64::MIN` yields no bound: the negation wraps
    /// back to a negative value, which is exactly the false-proof trap.
    #[test]
    fn negation_needs_a_bound_above_the_minimum() {
        let mut bounds = HashMap::new();
        bounds.insert(
            "a".to_string(),
            Bounds {
                low: None,
                high: Some(-1),
            },
        );
        let negated = Expr::Unary(UnOp::Neg, Box::new(Expr::Name("a".into())));
        let found = expr_bounds(&negated, &bounds);
        assert!(found.low.is_none() && found.high.is_none(), "{found:?}");

        // With a bounded-below operand the negation is exact.
        let mut known = HashMap::new();
        known.insert(
            "b".to_string(),
            Bounds {
                low: Some(-5),
                high: Some(-2),
            },
        );
        let negated = Expr::Unary(UnOp::Neg, Box::new(Expr::Name("b".into())));
        let found = expr_bounds(&negated, &known);
        assert_eq!(found.low, Some(2));
        assert_eq!(found.high, Some(5));
    }

    /// A false operand decides a conjunction even when the other side is symbolic.
    #[test]
    fn conjunction_short_circuits_to_false() {
        let state = State::default();
        let value = eval(
            &Expr::and(
                Expr::Int(0),
                Expr::binary(BinOp::Le, Expr::Int(0), Expr::Name("n".into())),
            ),
            &state,
        );
        assert_eq!(value, Value::Const(0));
    }

    /// Facts tighten intervals, including mirrored and negated comparisons.
    #[test]
    fn bounds_read_through_conjunction_and_negation() {
        let fact = Expr::and(
            Expr::binary(BinOp::Ge, Expr::Name("x".into()), Expr::Int(3)),
            Expr::Unary(
                UnOp::Not,
                Box::new(Expr::binary(
                    BinOp::Gt,
                    Expr::Name("x".into()),
                    Expr::Int(9),
                )),
            ),
        );
        let bounds = bounds_of(&[fact]);
        assert_eq!(bounds["x"].low, Some(3));
        assert_eq!(bounds["x"].high, Some(9));
    }
}
