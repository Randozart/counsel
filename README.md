# Counsel

Basic **pre/post-condition verification** for Rust, with the verification
machinery **authored in Briev** — the contract-first language from the
[`briev-lang`](https://github.com/) project — compiled to a native static
library and linked into a Cargo crate.

Positioned as **lighter than Kani**: it enforces obligations at runtime and (in
later phases) runs a cross-verification sampler. It is not a full
model-checker over the Rust memory model.

> **Status (Phase 0): scaffold spike.** The full build path —
> `brievc build bridge/counsel.bv --library` → bindings → Rust facade — is
> proven end-to-end. The contract prover agrees on the Briev machinery.

## How it works

```
bridge/counsel.bv   ← verification machinery, authored in Briev
        │  (brievc build --library → libcounsel.a)   [build.rs]
        ▼
src/bindings.rs     ← checked-in C-ABI bindings
        ▼
src/lib.rs          ← safe Rust facade
```

`build.rs` shells to `brievc` (selectable via the `BRIEVC` env var, default
`brievc` on PATH), compiles the `.bv` machinery to a static library, and links
it into the crate.

## Quick start

```bash
# Build linked library + machine tests
BRIEVC=/path/to/brievc cargo test
```

or, if `brievc` is already on your PATH:

```bash
cargo test
```

## Example

```rust
use counsel::{Verdict, add, within};

assert_eq!(add(3, 4), 7);          // Briev-computed
assert_eq!(within(5, 0, 10), Verdict::Pass);   // x in [lo, hi]
assert_eq!(within(11, 0, 10), Verdict::Fail("11 not in [0, 10]".into()));
```

## Phase plan

See [`docs/plans/2026-11-11-counsel-briev-verify.md`](docs/plans/2026-11-11-counsel-briev-verify.md)
and the feasibility study + decision record in
[`BRIEV-RUST-VERIFY-PLAN.md`](../BRIEV-RUST-VERIFY-PLAN.md).

- **Phase 0** — scaffold spike: build path proven. ✅
- **Phase 1** — Ambition A1: Rust-closure obligations (`#[contracts]` macro + `register_contract`/`run_checked` in Briev).
- **Phase 2** — cross-verification sampler (interleaved original-vs-shadow).
- **Phase 3** — Ambition A2: pure-Briev predicate AST.
- **Phase 4** — stretch slice of static proof (research).

## License

Apache-2.0 WITH LLVM-exception. Compiled output of Briev programs is exempt
from the notice/attribution obligations.