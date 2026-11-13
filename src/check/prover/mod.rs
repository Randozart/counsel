// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// prover — Counsel's own contract prover.
//
// Written for exactly the nodes Counsel emits, so it inherits no general-purpose
// language machinery: no loops, no calls, no type model. Calls are resolved on
// the `syn` tree and loops become separate obligations before a definition
// reaches here, which is why the walk is straight-line.
//
// It used to live in `briev-verify`, Briev's prover. Sharing it meant paying for
// a whole language's AST and gaining nothing, and it kept a dependency the crate
// did not need. The semantics preserved here are listed in
// `docs/plans/2026-11-14-standalone-prover.md`; the short version is that the
// facts stay in the entry state, a self-reference expands exactly once, an
// unsatisfiable model never refutes, and silence is never proof.

pub mod ast;
pub mod smt;
pub mod symbolic;
pub mod walk;

pub use ast::{BinOp, Contract, Defn, Expr, Stmt, UnOp};
pub use walk::{verify, Report, Verdict};
