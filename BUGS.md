# Counsel — Bug Log

Root causes and diagnostics. Follow the HARDCORE: state what failed, the
evidence, the resolution, and the path to permanent fix.

## 2026-11-11 — Cross-ABI function-pointer params SIGSEGV (A1 blocker)

**Status:** Resolved-by-avoidance (reliable redesign); **fix pending in Briev backend.**

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