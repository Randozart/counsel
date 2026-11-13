# Counsel — Bug Log

Root causes and diagnostics. Follow the HARDCORE: state what failed, the
evidence, the resolution, and the path to permanent fix.

## 2026-11-11 — Cross-ABI function-pointer params SIGSEGV (A1 blocker)

**Status:** MOOT (2026-11-14). The C ABI path was removed entirely: the runtime
half is now plain Rust, so nothing crosses an FFI boundary and there is no
callback slot to mismatch. Kept below as the record of the diagnosis, because it
is the reason the design does not use callbacks.

**Earlier status:** FIXED — briev-lang 43519b01. An exported Briev defn now adapts a host's flat C function pointer into its internal closure slot through a generated trampoline, so `run_checked` is authored in Briev again (323de0b) and the Rust facade passes `extern "C"` callbacks directly. The data-only workaround below remains valid and is still used by the predicate primitives.

### What failed

The Phase-1 (Ambition A1) plan passed Rust callbacks across the C ABI into a
Briev-authored orchestrator:

```briev
export defn run_checked(pre: (Int) -> Int, f: (Int) -> Int,
                        post: (Int, Int) -> Int, x: Int) -> Int {
    when pre(x) != 1 { term 1; };
    let ret: Int = f(x);
    when post(x, ret) != 1 { term 2; };
    term 0;
};
```

`brievc build --library` compiled this fine, and `brievc bindings ... rust`
emitted `run_checked(pre: {type}, f: {type}, post: {type}, x: i64)` — note the
**`{type}` placeholders**: the Rust binding generator does not render function
-ptr parameters. A hand-written binding with flat `extern "C" fn(i64) -> i64`
for `pre`/`f`/`post` produced a SIGSEGV at the first callback call.

### Evidence

The generated C binding declares a flat function pointer:

```c
int64_t run_checked(int64_t (*pre)(int64_t), int64_t (*f)(int64_t),
                    int64_t (*post)(int64_t, int64_t), int64_t x);
```

But the emitted LLVM (from `counsel.ll`) for `run_checked__briev_internal`
does **not** treat the param as that flat callback. It dereferences the param
as a slot and calls with a self/slot pointer plus the argument:

```llvm
%ac0 = ptrtoint ptr %arg0 to i64
%t2  = inttoptr i64 %ac0 to ptr
%t3  = load i64, ptr %t2          ; load the call target *from* the slot
%t4  = inttoptr i64 %t3 to ptr
%t6  = call i64 %t4(ptr %t2, i64 %arg3)   ; call (slot, arg) — closure convention
```

So the backend's expected callback ABI is a **closure/self convention**
(`fn(self_slot, arg)`), while the C binding generator advertises a **flat**
`int64_t (*)(int64_t)`. They disagree. A flat host fn pointer therefore
SIGSEGVs.

### Analysis (no special-treatment hack shipped)

Per the strict standards, we did **not** reverse-engineer and hand-maintain a
guessed slot-based binding — that would be fragile special treatment the crate
would carry forever. Instead:

- Reverted the `run_checked` fn-ptr export and its hand-written binding.
- **Reliable redesign:** Briev authors the *check predicates* over a pure
  `Int` ABI (`is_positive`, `is_nonneg`, `within`), which are proven safe; the
  Rust facade composes pre/body/post via the `contract(pre, body, post, x)`
  combinator, delegating each obligation's truth value to a Briev check.

### Path to permanent fix (Briev backend)

Either (a) make `emit_call_ptr` call the flat `int64_t (*)(int64_t)` ABI that
the binding generators advertise, or (b) teach the Rust binding generator to
render the slot/closure signature (`fn(ptr, i64) -> i64`) and have Counsel
build a thunk. Until then, cross-ABI callbacks are not a supported surface;
pure Int-ABI checks are.

## 2026-11-11 — Briev: dangling `ret` when every path terminates in guards

**Status:** documented (reported upstream); worked around in the translator.

### What failed

Translating `if a > 5 { a } else { 0 }` naively (both arms behind a `when`
guard) makes the *Rust* test suite fail with an LLVM verifier error:

```
opt: .../counsel_static.ll:6537:11: error: use of undefined value '%t17'
  ret i64 %t17
error: opt failed for '.../counsel_static.ll'
```

### Minimal repro (Briev only, no Rust involved)

```briev
// FAILS — every path terminates inside a `when`, no trailing `term`
defn f(a: Int) [a >= 0][term >= 0] -> Int {
    when a > 5 { term a; };
    when a <= 5 { term 0; };
};
```

```briev
// COMPILES — identical semantics, but a trailing `term` exists
defn g(a: Int) [a >= 0][term >= 0] -> Int {
    when a > 5 { term a; };
    term 0;
};
```

### Evidence

Generated IR for `f`:

```llvm
  guard.then13:
  %t15 = add i64 0, 0
  ret i64 %t15
  guard.end13:
  ret i64 %t17        ; %t17 is never defined
```

The emitter's implicit-return fallback references a result register that was
never materialised, so `opt` rejects the module.

### Analysis

`--verify-bodies` itself was *correct* here (no contract violation reported);
the failure is codegen, in the LLVM backend's guard/termination handling when
a function body has no trailing statement after fully-terminating guards.

### Workaround shipped

Counsel emits the else body **unguarded** when both arms terminate
(reaching it implies the guard failed), which is both simpler and avoids the
shape. When the then-arm can fall through, the inverted guard is kept.

### Path to permanent fix (Briev backend)

Emit only reachable terminators: when the guard's end block is unreachable
every path has already returned, emit `unreachable` (or nothing) instead of a
`ret` on an undefined register.
