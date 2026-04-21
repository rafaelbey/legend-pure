---
name: Parity semantics to preserve
description: Subtle Pure semantics the Rust port must preserve to match the Java reference; easy to miss on the happy path
type: project
---

**Function overload dispatch.** Pure allows multiple functions with the same simple name differentiated by parameter types and multiplicities. Java computes mangled names like `plus_Integer_MANY__Integer_1_`. The Rust port does this at declaration time (`crates/pure/src/pipeline.rs:258-272`) and dispatches with:
1. Simple-name lookup in root + all imports
2. Filter by `parameters.len() == arg_count` (AST count, not lowered count — a lowering failure must not silently change arity)
3. `narrow_candidates_by_type` scores by exact-type +3, subtype +1, generic +0, eliminate on incompatible; and per-multiplicity specificity score (PureOne +4, ZeroOrOne +3, OneOrMany +2, ZeroOrMany +1).
See `crates/pure/FUNCTION_DISPATCH.md`.

**Mangled FQN is the runtime dispatch key.** The evaluator reads `model.get_node(id).name` as the native-registry key (`crates/runtime/src/eval.rs:387`). Any change to the mangling format must be matched in the native registration strings, or dispatch silently falls through to `find_by_prefix`.

**Multiplicity coercion semantics.** `[1]` is scalar, `[0..1]` is optional, `[*]` is collection, `[1..*]` is non-empty collection, `[n..m]` is a range. At function-call boundaries the interpreter uses `Value::to_one()` / `to_zero_one()` / `to_collection()` to coerce (`crates/runtime/src/value.rs:337-420`). A collection of length 1 is coercible to `[1]` (unlike naive "collections are collections").

**Deferred argument evaluation** for `if`, `and`, `or` (and likely `match`). The native trait has `defer_execution() -> bool` (`crates/runtime/src/native.rs:145`). When true, the evaluator wraps each arg in a zero-param `LambdaClosure` and passes it; the native calls `ctx.eval_lambda()` on the taken branch. Mirrors Java's `deferParameterExecution()`. Skipping deferral breaks short-circuit semantics (e.g., `if(a != 0, | b / a, | 0)` would evaluate the divide-by-zero branch).

**Milestoning rewrite** (`<<temporal.businesstemporal>>` etc.). Java's `PostProcessor.populateTemporalMilestonedProperties()` mutates classes: generates `allVersions`, `allVersionsInRange`, date-range properties, and modifies association resolution. The Rust port has not yet implemented this (no traces in `crates/pure/src/pipeline.rs`). Any class with temporal stereotypes will compile but miss synthetic properties — downstream tests that use `.businessDate(%2020)` will fail silently or with UnresolvedElement.

**Association property injection.** Java's `populatePropertiesFromAssociations()` injects the two association properties into their respective owner classes. The Rust port handles this via `association_property_index` (derived index) rather than mutation — but `all_properties(id)` must compose declared + inherited + association, and snap-tests should cover cross-package associations and AssociationProjection.

**PCT (Platform Compatibility Testing) is the cross-engine parity contract.** Functions marked `<<PCT.function>>` in Pure are tested via `<<PCT.test>>` which take an adapter parameter. Java runs them through both `FunctionExecutionInterpreted` and the generated compiled engine; divergence fails the build. Rust port must eventually run PCT tests through its interpreter + (future) hybrid-compiled backend and stay consistent with the Java runs. Exclusion mechanisms:
- Pure-side: `{test.excludePlatform = 'Java compiled'}` tag on the test function
- Java-side: `expectedFailures` list in `Test_Compiled_*_PCT` / `Test_Interpreted_*_PCT` runners
Rule: intentional platform divergence → Pure-side; known bug → Java-side with tracking issue.

**Lazy call stack.** Java pushes/pops `StackFrame` at every function boundary. Rust's `Evaluator` has NO call stack field — on error, each recursive `eval` layer wraps via `.map_err(|e| e.with_frame(...))` (`crates/runtime/src/eval.rs:531-536`). Happy path cost = zero; error path materializes frames as it unwinds. This means any benchmark comparison that assumes the interpreter "maintains a stack" will be confused — and any per-call profiling needs explicit hooks (`EvalHooks::enter_function/leave_function`).

**`Executor` is not `Send`**. Deliberate: `im_rc::Vector` uses `Rc`. Share models across threads with `Arc<PureModel>`, spawn a per-thread `Evaluator`. Don't try to move a `Value::Collection` across threads.

**Hard/soft dependency classification** (supertypes = hard, property types = soft) allows cyclic data models (`Person { company: Company }` ↔ `Company { employees: Person }`) while detecting cyclic inheritance as a compile error. Any refactor to dependency extraction must preserve this split (`crates/pure/src/pipeline.rs:507-527`).

**`SourceInfo` on every semantic node.** Runtime errors must report back to user source even though the AST may have been dropped. Any new semantic node MUST carry `SourceInfo`; any new expression variant must populate `ValueSpec.source_info`.
