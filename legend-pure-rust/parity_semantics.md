# Pure-Rust port: parity semantics

This document records *deliberate divergences* between the Rust port's
behaviour and the upstream Java Pure implementation. By default the
Rust compiler holds Java parity; deviations are gated behind opt-in
flags so the platform PCT corpus stays green.

If a behaviour isn't listed here, the Rust port targets exact Java
semantics. Discrepancies you find should be filed as parity bugs
unless the cause is documented in this file.

## Table of contents

- [Strict-mode inference](#strict-mode-inference)

## Strict-mode inference

**Toggle:** `LEGEND_PURE_STRICT_INFERENCE=1` env var, or
`legend_pure_parser_pure::strict_mode::with_strict_mode(true, || …)`
for tests / programmatic opt-in. Default OFF — the platform compile
runs in lenient (Java parity) mode.

**Plan reference:** Steps 3f / 3g in
`~/.claude/plans/do-we-have-enought-quiet-swing.md`.

### Why we diverge here

Java's `FunctionExpressionProcessor` silently widens mismatched-arg
types via `findBestCommonGenericType`'s covariant LUB at
`register():467-480`. Java itself flags this as a known weakness
(comment at `register():474-478`: "should LUB to Any but currently
doesn't"). Practical consequences:

- `eval(f:Function<{Integer→String}>[1], 'wrong')` — T binds Integer
  authoritatively from the FunctionType slot; the constraint slot
  `param:T` LUBs Integer + String → Any, then `param:Any` accepts the
  String silently.
- `pick<T>(1, 'x')` — both T-binding slots LUB to Any. No error fires.
- `someFn<T>(): T[1]` called from a non-parametric function body —
  T can't bind from any sibling arg; Java would emit "type parameter
  T was not resolved" at the *outermost* context only
  (`TypeInference.java:87-89`, gated on `getParent() == null`); inside
  a function body the check stays silent.
- Lambda params with `Generic(T)` expected type and T not in scope —
  Java emits "cannot infer lambda parameter type"
  (`TypeInference.java:116, :127`); we currently fall through to
  `Unresolved` placeholder and dispatch carries on.

Strict mode trades the lenient permissiveness for loud diagnostics.
Use cases: porting / migrating existing Pure source, CI quality gates,
catching dispatch ambiguities that Java would silently widen.

### What strict mode does

| Site | Default (Java parity) | Strict mode |
|---|---|---|
| Per-arg type compatibility check (`infer.rs::validate_call_arguments`) | Checks against the raw declared `param.type_expr` — `Generic(T)` permissive. | Computes per-arg bindings *excluding* the arg under check (replaces it with an `Unresolved`-typed placeholder so `bind_type`'s LUB-merge doesn't widen back to Any), substitutes the param type with those bindings, and rejects incompatible args. Catches the `eval(f, 'wrong')` case. |
| Unresolved generics in the substituted return type (`infer.rs::infer_function_call`) | Silent. Java is gated on `getParent() == null`; we don't model that gate exactly. | Walks the substituted return for surviving `Generic(name)` whose `name` isn't in the enclosing fn's `type_params_in_scope`. Emits `UnresolvedTypeParameter`. |
| Unresolved multiplicity in the substituted return | Silent. | Same idea as above for `Variable(name)` not in `mult_params_in_scope`. Emits `UnresolvedMultiplicityParameter`. |
| Lambda param expected as `Generic(T)`-not-in-scope (`lower/lambda.rs::lower_lambda_parameters`) | Falls through to `Unresolved` placeholder. The eager `CannotInferLambdaParameterTypes` only fires when both declaration AND expectation are missing. | Treats `Generic(name)` where `name` ∉ `ctx.type_parameters` as a lambda inference failure. Emits `CannotInferLambdaParameterTypes` for the affected param. |

### Caveats

- **Strict mode is strictly more diagnostics than Java emits.** Java's
  unresolved-param error is gated on outermost context
  (`getParent() == null`); we report at every call site. This is
  intentional — porters benefit from loud signal over Java's silent
  drift.
- **Per-arg excluding-self is approximate.** It mirrors Java's
  authoritative-vs-constraint two-branch dispatch
  (`FunctionExpressionProcessor:567-594`) without porting the full
  `TypeInferenceContext::register(merge: bool)` algorithm. Step
  3d-cont in the plan would land that — currently deferred because the
  excluding-self trick works for every test we've identified, and the
  prior two spikes attempting the full port both regressed
  fold-style chains.
- **Multiplicity bindings re-use the full bindings**, not the
  excluding-self set. Multiplicity LUB stays in the range lattice and
  doesn't suffer the same widening pathology as type LUB; the
  strict-mode arg-type check is the load-bearing divergence.

### Test pinning

Each strict divergence is locked in pairs of tests in
`crates/pure/tests/inference_context.rs`:

| Lenient pin (default) | Strict pin (opt-in) |
|---|---|
| `tic_eval_wrong_arg_currently_lenient_pre_strict_mode` | `tic_eval_wrong_arg_strict_mode_errors` |
| `tic_unbound_t_at_nested_call_currently_silent` | `tic_unbound_t_at_nested_call_strict_mode_errors` |
| `tic_unbound_multiplicity_at_nested_call_currently_silent` | `tic_unbound_multiplicity_at_nested_call_strict_mode_errors` |
| `tic_lambda_param_with_unbound_t_currently_silent` | `tic_lambda_param_with_unbound_t_strict_mode_errors` |

If Java semantics ever change such that the lenient pins should flip,
the test failure is the alarm: read its doc-comment for the migration
recipe (typically: rewrite `expect("compiles silently")` →
`expect_err(...)` plus the relevant diagnostic kind, or move the test
under a `with_strict_mode(true, || ...)` wrapper).

If strict mode itself grows new behaviour (e.g. Step 3d-cont lands the
full TypeInferenceContext), update the strict pins to assert the
sharper diagnostic shape and add new tests pinning the unaffected
fold-style / recursive-generic / collection-of-lambdas patterns.

### Shipping the toggle

The thread-local `with_strict_mode` is the test/library entry. For
process-wide opt-in (CI runs, dedicated migration jobs), set
`LEGEND_PURE_STRICT_INFERENCE=1`. The toggle is read at the call-site
hot path, so flipping the env var mid-process has no effect — set it
before invoking the binary.
