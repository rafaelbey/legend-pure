# Pure-Rust port: parity semantics

This document records *deliberate divergences* between the Rust port's
behaviour and the upstream Java Pure implementation. The Rust compiler
targets exact Java semantics by default; the items below are the
exceptions.

If a behaviour isn't listed here, the Rust port targets exact Java
semantics. Discrepancies you find should be filed as parity bugs
unless the cause is documented in this file.

## Table of contents

- [Inference: prescriptive divergences](#inference-prescriptive-divergences)

## Inference: prescriptive divergences

These behaviours are **always on**. There is no toggle — they ship as the
default Rust-port semantics. The history: an earlier `with_strict_mode`
toggle (and `LEGEND_PURE_STRICT_INFERENCE` env var) gated them off by
default, but once the platform reached zero errors under strict, the
toggle was removed and strict became default.

### Why we diverge

Java's `FunctionExpressionProcessor` silently widens mismatched-arg
types via `findBestCommonGenericType`'s covariant LUB at
`register():467-480`. Java itself flags this as a known weakness
(comment at `register():474-478`: "should LUB to Any but currently
doesn't"). Practical consequences in Java:

- `eval(f:Function<{Integer→String}>[1], 'wrong')` — T binds Integer
  authoritatively from the FunctionType slot; the constraint slot
  `param:T` LUBs Integer + String → Any, then `param:Any` accepts the
  String silently.
- `someFn<T>(): T[1]` called from a non-parametric function body —
  T can't bind from any sibling arg; Java emits "type parameter T was
  not resolved" only at the *outermost* context
  (`TypeInference.java:87-89`, gated on `getParent() == null`); inside
  a function body the check stays silent.
- Lambda params with `Generic(T)` expected type and T not in scope —
  Java emits "cannot infer lambda parameter type"
  (`TypeInference.java:116, :127`) only at outermost.

The Rust port instead surfaces these consistently. Use cases: porting
existing Pure source, CI quality gates, catching dispatch ambiguities
that Java would silently widen.

### What the Rust port does (always)

| Site | Java | Rust port |
|---|---|---|
| Per-arg type compatibility check (`infer.rs::validate_call_arguments`) | Checks against the raw declared `param.type_expr` — `Generic(T)` permissive. | Substitutes `param.type_expr` through the call's `ty_auth` bindings (only T's sourced from a structural FunctionType slot, not from a top-level Generic-typed arg), then checks compatibility. Catches the `eval(f, 'wrong')` case while letting `compare(1, 'a')`/`compare(1, 2.2)` keep their Java-style covariant-LUB-to-common-supertype behaviour. |
| Unresolved generics in the substituted return type (`infer.rs::infer_function_call`) | Silent (gated on `getParent() == null`). | Walks the substituted return for surviving `Generic(name)` whose `name` isn't in the enclosing fn's `type_params_in_scope`. Emits `UnresolvedTypeParameter`. |
| Unresolved multiplicity in the substituted return | Silent. | Same idea for `Variable(name)` not in `mult_params_in_scope`. Emits `UnresolvedMultiplicityParameter`. |
| Lambda param expected as `Generic(T)`-not-in-scope (`lower/lambda.rs::lower_lambda_parameters`) | Falls through to `Unresolved` placeholder; eager check only when both declaration AND expectation are missing. | Treats `Generic(name)` where `name` ∉ `ctx.type_parameters` as a lambda inference failure. Emits `CannotInferLambdaParameterTypes`. |

### Auth/Constraint binding distinction

The load-bearing mechanism is the auth-vs-constraint split (Step "auth"
in the inference rebuild). `GenericBindings` carries two binding maps:

- `ty_auth` — Generic(T) bindings sourced from a *structural FunctionType
  slot* (Pure-invariant).
- `ty` — same, plus bindings from top-level Generic-typed args
  (Java-style covariant LUB).

The per-arg type-compatibility check substitutes via `ty_auth` only;
constraint-only T's stay `Generic("T")` and `is_type_compatible`'s
wildcard arm accepts. That's how `eval(intFunc, 'wrong')` catches
without false-positives on `compare(1, 'a')`.

`bind_type_with_mode` provides the leaf-level switch. The two-branch
dispatch in `infer_generic_bindings` classifies args by pass-1
convergence and switches between `RegisterMode::Authoritative`
(any arg unconverged → `potentiallyUpdate…`-style first-wins) and
`RegisterMode::Constraint` (all converged → `update…`-style LUB-merge).

### Test pinning

Each divergence is locked in `crates/pure/tests/inference_context.rs`
by tests with the `tic_*_strict_mode_*` prefix (the name preserves the
historical "strict" framing — the tests now run under default
semantics). Companion lenient pins document Java parity behaviours
that the Rust port intentionally keeps (e.g.
`tic_pick_t_t_with_unrelated_args_lubs_silently` — `pick<T>(1, 'x')`
LUBs to Any silently, matching Java).

If a divergence is rolled back in the future, the failing strict pin is
the alarm: read its doc-comment for the migration recipe.
