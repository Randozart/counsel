// oracle_fixtures/catalog.rs — the shape-oracle corpus.
//
// Exercised by tests/type_oracle.rs. This file is valid Rust and is INCLUDED as a
// module of the test crate (see `mod catalog` there), so rustc is the heavy-tool
// oracle: if this stops being well-typed Rust, the test crate stops compiling. Tests
// then assert that Counsel's resolver (the lightweight rule) agrees with the facts
// rustc has already validated.

/// Declared order matters: the resolver's tag for a variant is its index here.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Accel {
    Try,
    Force,
    TryAll,
    TryAllForce,
}

pub enum Opt {
    Some(i64),
    None,
}

/// A struct is not an enum: `Rect::x` must never resolve as a variant tag.
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: u8,
}

impl Accel {
    /// A method on the enum; tests assert it does not disturb variant tagging.
    pub fn forces(&self) -> bool {
        matches!(self, Accel::Force | Accel::TryAllForce)
    }
}
