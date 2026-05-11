<!--
  Copyright 2026 Goldman Sachs

  Licensed under the Apache License, Version 2.0 (the "License");
  you may not use this file except in compliance with the License.
  You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

  Unless required by applicable law or agreed to in writing, software
  distributed under the License is distributed on an "AS IS" BASIS,
  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
  See the License for the specific language governing permissions and
  limitations under the License.
-->

# TODO — Developer Inbox

Drop-in inbox for bug reports and feature requests against the Rust workspace
(`legend-pure-rust/`). The file is **append-only for humans** and **edited in
the agent-audit lane by agents** who pick items up.

- **`BACKLOG.md`** is the curated, prioritised work plan. Once an item here is
  triaged into the plan, an agent should reference its BACKLOG entry (or open
  one) and close the TODO entry with `Status: Migrated → BACKLOG`.
- **`TODO.md`** (this file) is for raw, unfiltered feedback so nothing gets
  lost. Be terse. Don't try to solve it — just describe it.

---

## How to file an item (developers)

Copy the template below, fill it in, and append it under `## Open`. One `###`
heading per item. Give it an `ID` of the form `T-YYYYMMDD-NN` so agents can
reference it in commits.

```markdown
### T-YYYYMMDD-NN — <short title>

- **Type:** bug | feature | refactor | docs | perf | parity-gap
- **Area:** parser | compiler | runtime | cli | jni | protocol | dsl-<name> | stress | docs
- **Priority:** P0 | P1 | P2 | P3   <!-- optional; agents will re-triage -->
- **Reporter:** <github handle or name>
- **Filed:** YYYY-MM-DD

**Summary**
One or two sentences. What is wrong, or what should exist.

**Repro / Context**           <!-- bugs: how to reproduce; features: motivating use case -->
- Command(s) run, file path(s), commit hash if relevant
- Expected vs. actual (bugs) OR acceptance criteria (features)
- Java-Pure behaviour reference, if this is a parity gap

**Notes**                     <!-- optional: links to related code, prior attempts, gotchas -->
- file/path.rs:123
- See BACKLOG row "<title>"

---
<!-- agent-audit:start id=T-YYYYMMDD-NN -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
<!-- agent-audit:end -->
```

### Field hints

- **Type — `parity-gap`**: behaviour differs from the upstream Java stack
  (`legend-pure-core/.../platform/`). Always preferred over `bug` when the
  divergence is the actual signal.
- **Area**: use the crate name where the fix probably lives. If unsure, leave
  it blank — first triage will set it.
- **Priority**: a hint, not a contract. Triage may downgrade or escalate.
- **Repro**: minimal `.pure` snippet beats a full file. Pasted output of
  `cargo test -p … <name>` is gold.

---

## How agents work this file

Read top-to-bottom on every visit:

1. **Pick the highest-priority `Open` item** (or one a teammate just escalated
   via comment). Skip items whose `Status` line marks them blocked / claimed
   by another agent in the last 24h.
2. **Claim it** by appending to its `agent-audit` block:
   `- YYYY-MM-DD HH:MMZ — claimed by <agent-id> — <one-line intent>`.
3. **Investigate and act.** Honour the workspace's correctness policy in
   `CLAUDE.md` (§ *Correctness policy: no tactical test-pass hacks*) and the
   `.claude/.../MEMORY.md` feedback rules — especially: no tactical hacks,
   no special-casing by name, no parallel-vec source info, validators next
   to the data, commits need explicit human sign-off.
4. **Log progress** in the `agent-audit` block. One bullet per substantive
   step: what you ran, what you found, what you changed. Reference file
   paths as `crate/path.rs:LINE`.
5. **Set Status** in the audit block:
   - `Investigating` — actively looking
   - `Reproduced` — confirmed the report
   - `Cannot reproduce` — and what was tried
   - `Fix proposed` — link the diff / commit / branch
   - `Fix landed` — commit SHA on `legend-pure-rust` (or the merging branch)
   - `Blocked` — and why (waiting on … / depends on …)
   - `Migrated → BACKLOG` — for items too large to fix inline; reference
     the BACKLOG row title and move the item to `## Closed` below
   - `Wontfix` — and why; move to `## Closed`
   - `Duplicate of T-…` — and move to `## Closed`
6. **When done**, move the entire item block (developer block + audit block)
   to `## Closed`, with the final Status line at the bottom.

### Audit-block format

```markdown
<!-- agent-audit:start id=T-YYYYMMDD-NN -->
- 2026-05-10 14:02Z — claimed by claude-opus-4-7 — reproducing locally
- 2026-05-10 14:18Z — reproduced via `cargo test -p legend-pure-pure foo_bar`;
  root cause looks like `pipeline.rs:1183-1198` not seeding `$this` for X.
  Status: Investigating
- 2026-05-10 15:01Z — patch on branch `fix/T-20260510-01`; passes new test.
  Status: Fix proposed (commit abc1234)
<!-- agent-audit:end -->
```

### Rules for agents (hard)

- **Never edit the developer block.** Corrections go in the audit block as
  `Clarification: …`. The developer block is the original report.
- **Never delete a TODO item.** Close it by moving to `## Closed` with a
  terminal Status.
- **Don't auto-commit.** Per workspace feedback memory, `continue`/`next`
  does NOT authorise a git commit. Surface the diff, wait for explicit
  sign-off.
- **Don't batch-claim.** One claim at a time per agent unless a teammate
  explicitly asks for a sweep.
- **Cite primary sources.** When the question is "what does Java do?",
  point at a path under `legend-pure-core/.../platform/` or a Java class
  in the upstream tree — not memory, not folklore.

---

## Open

<!-- New items go here. Newest at the top. -->

### T-20260511-01 — Property default value not type-checked against declared property type

- **Type:** parity-gap
- **Area:** compiler
- **Priority:** P1
- **Reporter:** Rafael
- **Filed:** 2026-05-11

**Summary**
A class property's default-value expression is not checked for type
compatibility against the property's declared type+multiplicity. A
default value of an incompatible type is silently accepted. Java Pure
rejects this.

**Repro / Context**

```pure
Class abc::Foo
{
  prop3 : Integer[1] = '';
}
```

- Declared property type: `Integer[1]`.
- Default value: `''` — a `String[1]` literal.
- Expected: compile error — default-value expression type
  `String[1]` is not compatible with declared property type
  `Integer[1]`.
- Actual: compiles clean.

**Notes**
- Sweep symmetric cases — all four should produce compile errors:
  (a) wrong type (e.g. `prop : Integer[1] = ''`),
  (b) wrong multiplicity (e.g. `prop : Integer[1] = [1, 2, 3]`,
      `prop : Integer[1..*] = []`),
  (c) wrong nested generic (e.g. `prop : List<String>[1] = ^List<Integer>(…)`),
  (d) subtype OK in the covariant direction
      (`prop : Number[1] = 1`), but not the contravariant one.
- Same check almost certainly belongs on **qualified properties**'
  body expressions (return type vs. body's inferred type) and on
  **function bodies** (return type vs. last expression's type). The
  TODO is just about property defaults; sweep adjacent sites for
  the same hole as part of triage.
- Property default values are lowered in Pass 2b
  (`crates/pure/src/pipeline.rs::pass_define_class_bodies`) per
  the comment at pipeline.rs:1674-1678 ("Default-value bodies are
  lowered in Pass 2b… so type-based dispatch in any operator/function
  call inside a default value sees real return types"). That's the
  same seam where the type-compat check should fire: after the
  default-value expression is lowered (so its inferred type is known)
  and against the property's already-resolved
  `type_expr + multiplicity` (already on the `Property` shell from
  Pass 1).
- Reuse `crate::infer::is_type_compatible_structural` (or whatever
  the canonical compat function is — verify, don't trust): it's the
  same routine that gates `cast(@T)`, function-call argument binding,
  etc. The new validator should call it, not re-implement compat.
- Error shape: parity with the analogous Java diagnostic. Likely
  something like `"Default value of property 'X' has type Y[m], but
  property declared as Z[n]"` — confirm against Java's
  `M3PropertyValidator` / `ClassValidator` family before writing.
- This bug is in the same family as T-20260510-03 (FunctionType arity/types
  not checked when binding lambda argument) — both are "the structural
  compat function exists but isn't called at this binding site". Fixing
  one may share the call-site discipline needed for the other.

<!-- agent-audit:start id=T-20260511-01 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
<!-- agent-audit:end -->

### T-20260510-04 — Repo visibility pattern not enforced against declared FQNs

- **Type:** parity-gap
- **Area:** compiler (repo loader / build)
- **Priority:** P1
- **Reporter:** Rafael
- **Filed:** 2026-05-10

**Summary**
A repo's visibility/ownership `pattern` regex is not validated against the
FQNs of the elements it ships. Adding an element whose FQN doesn't match
the pattern is accepted silently. Java Pure rejects this at load time.

**Repro / Context**
Repo descriptor with `pattern = "((meta)|(system)|(apps::pure))(::.*)?"`
ships `abc::Class`. The FQN `abc::Class` does not match — no element
under `abc::…` is in the pattern's language. Expected: build-time error
("element abc::Class is not within repo X's pattern"). Actual: no error.

**Notes**
- This is the layering primitive: a repo's `pattern` defines *which*
  FQNs it is permitted to own. Without enforcement, repos can ship
  anything and the cross-repo visibility check (T-style "is this
  reference declared as a dependency?") loses its meaning, because
  there's no reliable answer to "which repo does this element belong
  to?".
- Related BACKLOG row: "Repo descriptors + manifest (Pure-graph
  composition) | P1" — step (4) in that plan is exactly this
  validation. Today's `crates/core-platform-pure/build.rs` walks
  hand-listed directories and never consults a regex. Even before the
  full manifest-driven loader lands, the pattern check can be wired in
  against the current hardcoded repo descriptors.
- Where the check should live: at the loader / build-script seam, the
  moment a `.pure` file's top-level FQNs are known. Build-time error
  with the offending FQN + repo name + pattern, not a deferred
  compile-pass diagnostic. (Compile-pass is fine as a fallback for
  dynamically loaded slices, but build-time is the strong gate for
  embedded repos.)
- Existing cross-repo visibility validators to cross-reference for
  pattern + error-shape consistency:
  `crates/dsl-mapping/src/compiler.rs::validate_repo_visibility` and
  `crates/dsl-relational/src/compiler.rs::validate_repo_visibility`
  (memory: "DSL Path runtime architecture" / Mapping DSL state, Phase
  2 / Phase D). Those handle *cross-repo references*; this bug is the
  prior check that an element is in its own repo at all.
- Sweep: also enforce that two repos don't both claim the same FQN
  (overlap check across `pattern`s) — a related but separable
  validator. Don't bundle into this fix unless cheap.

<!-- agent-audit:start id=T-20260510-04 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
- 2026-05-11 02:30Z — claimed by claude-opus-4-7[1m] — wiring repo
  pattern enforcement (Stage 2 of repo descriptors). Status: Investigating
- 2026-05-11 03:15Z — design (revised after red-team test): placement is
  **cross-chunk** in `validate(model)`, NOT hydration-inline. Initial
  plan was hydration-inline at `pipeline::hydrate_element_signature`,
  but DSL extensions (Mapping / Database / Diagram / Graph / TDS /
  Path) allocate `Element::DSLInstance` rows directly in their
  `define()` paths and bypass that hook entirely. A red-team test in
  `crates/dsl-mapping/tests/repo_pattern_dsl_smoke.rs` confirmed the
  gap: a `Mapping abc::M (...)` under `/platform/abc/m.pure` was
  silently accepted while the corresponding `Class abc::Foo` correctly
  errored. Cross-chunk in `validate(model)` walks every chunk's
  top-level elements uniformly — same shape as `validate_repo_visibility`
  at validate.rs:359 — and matches Java's `RepositoryPackageValidator`
  which runs as a post-compile `MatchRunner`.
- 2026-05-11 03:45Z — implementation:
    * `regex` added to workspace deps + `pure` and `snapshot-builder`
      crate Cargo.toml.
    * `crates/pure/src/visibility.rs`: new `RepoPattern { source,
      compiled }`, `RepoPatternMap`, `compile_repo_pattern` (anchors as
      `^(?:…)$` to mirror Java `Matcher.matches()`).
    * `crates/pure/src/model.rs`: new `PureModel.repo_patterns` field +
      empty default in `new()`.
    * `crates/pure/src/error.rs`: new `CompilationErrorKind::PackageNotInRepoPattern
      { package, repo, pattern }`.
    * `crates/pure/src/validate.rs`: new
      `validate_repo_pattern_membership(model)` cross-chunk pass
      emitting Java-parity message
      `"Package <pkg> is not allowed in <repo>; only packages matching <pattern> are allowed"`.
      Wired into `pub(crate) fn validate(model)` next to
      `validate_repo_visibility`. Uses `node.parent_package` + `fqn_path::package_path`
      to derive the package FQN — avoids the whole-tree scan that
      `element_fqn_path` does.
    * `crates/snapshot-builder/src/lib.rs`: `LoadedDescriptor.compiled_pattern`
      stored at load time; descriptor regex compile failure surfaces
      as `BuildError::Descriptor`; new `populate_repo_patterns` runs
      next to `populate_repo_visibility`.
    * `crates/lsp/src/diagnostics.rs` and
      `crates/core-platform-pure/tests/categorize_errors.rs`: new
      exhaustive-match arm for the new error kind.
    * `crates/pure/tests/repo_pattern_smoke.rs`: 6 integration tests
      (positive / negative / empty-map / unknown-use-site / anchored
      `metadata` / function declaration outside pattern). All green.
    * `crates/dsl-mapping/tests/repo_pattern_dsl_smoke.rs`: 2 DSL
      integration tests pinning that `Element::DSLInstance` rows are
      also gated (Mapping declared at `abc::M` under `/platform/...`
      with platform pattern → violation; in-pattern Mapping → no
      violation). Red-team guarantee against future placement
      regressions.
- 2026-05-11 04:10Z — verification:
    * `cargo build --workspace` green.
    * `cargo test -p legend-pure-parser-pure -p legend-pure-snapshot-builder
      -p legend-pure-core-platform -p legend-pure-lsp` green incl. doctests.
      Notably `snapshot-builder/tests/smoke.rs::builds_platform_purem_and_round_trips`
      passes — exercises the new validator end-to-end against real
      platform descriptors, so platform sweep is implicitly clean
      (no shipped `.pure` source declares an FQN outside its repo's pattern).
    * `cargo fmt --check` on all touched files: clean (per-file `rustfmt`).
    * `cargo clippy --lib -p legend-pure-parser-pure -p legend-pure-snapshot-builder
      -p legend-pure-lsp -- -D unwrap_used -D expect_used`: clean (warnings
      pre-date this change).
    * `./scripts/check-copyright.sh`: clean (433 files).
    * Two pre-existing failures in `legend-pure-dsl-mapping-runtime::heap_population_smoke`
      under nextest parallel execution are NOT caused by this change —
      they pass when run in isolation, and the validator is a no-op in
      those tests (no descriptor populates `repo_patterns`, source path
      has no `/<repo>/` prefix). Out of scope.
  Status: Fix proposed (working tree on branch `legend-pure-rust`,
  uncommitted; awaiting explicit sign-off per workspace commit rule).
<!-- agent-audit:end -->

### T-20260510-03 — FunctionType arity/types not checked when binding lambda argument

- **Type:** parity-gap
- **Area:** compiler
- **Priority:** P1
- **Reporter:** Rafael
- **Filed:** 2026-05-10

**Summary**
When an argument of `Function<{P1,…→R}>` type is passed a lambda, the
compiler does not check that the lambda's parameter list (count + types)
or return type match the declared FunctionType. A lambda of the wrong
arity / wrong parameter type is silently accepted.

**Repro / Context**

```pure
function <<PCT.test>> meta::pure::functions::math::tests::pow::testComplexPow<Z|y>(
    f:Function<{Function<{->Z[y]}>[1]->Z[y]}>[1]   // $f takes a zero-arg lambda returning Z[y]
):Boolean[1]
{
    // OK — zero-arg lambda
    assertEq(16.0, $f->eval(|2->pow(pow(2,2))));

    // WRONG — one-arg lambda `a:String[1]|...` passed where a zero-arg
    // lambda is required. Should not compile.
    assertEqWithinTolerance(
        182.88729271224377725957310758531093597412109375,
        $f->eval(a:String[1]|pow(3.33,4.33)),
        0.0000000000001);
}
```

- Expected: compile error on the second call — argument's FunctionType
  `{String[1]→…}` does not match the parameter's `{→Z[y]}` (parameter
  count mismatch; would also mismatch on parameter type / return type
  if arity were the same).
- Actual: both calls compile.

**Notes**
- Generalisation: FunctionType compatibility must check **all three**
  axes — parameter count, each parameter type+multiplicity, and the
  return type+multiplicity. Plus the generic/multiplicity substitution
  set on the way in. The unification rule for `Function<{A→B}>` vs.
  `Function<{A'→B'}>` is per-position covariant/contravariant per the
  language spec; whatever the spec says, the check must actually run.
- Related but distinct: BACKLOG row "Full generic unification (`Z`
  propagation)" and the un-ignored
  `function_type_higher_order_wrong_inner_type_errors` test. That work
  validated nested *type-argument* compatibility via
  `is_type_compatible_structural`. This bug is about validating the
  **FunctionType shell itself** at the binding site of a lambda
  argument — likely the same compat function needs to recurse into
  the lambda's actual parameters/return, not just trust nominal
  Function-ness.
- Suspect: argument→parameter binding in dispatch / higher-order eval
  treats any callable value as compatible with `Function<{…}>` once
  the outer name `Function` matches. Look at `crates/pure/src/infer.rs`
  and the `eval`/`apply` dispatch path; cross-check against
  `is_type_compatible_structural` to see whether it descends into
  `FunctionType` parameter/return positions or stops at the named
  shell.
- Sweep symmetric cases: (a) wrong return type on the inner lambda,
  (b) wrong multiplicity on a parameter, (c) extra/missing parameters,
  (d) parameter type that isn't even a subtype of the declared one.
  All four should produce compile errors.
- As with T-20260510-02, the platform `.pure` source for this PCT test
  will need fixing once the validator fires (the example here is
  intentionally malformed by the reporter). Compiler fix lands first.

<!-- agent-audit:start id=T-20260510-03 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
<!-- agent-audit:end -->

### T-20260510-02 — Undeclared multiplicity parameter in function signature compiles

- **Type:** parity-gap
- **Area:** compiler
- **Priority:** P1
- **Reporter:** Rafael
- **Filed:** 2026-05-10

**Summary**
A function declares its generic and multiplicity parameters in `<…|…>`, but
the body of the signature references a multiplicity name that was never
declared. The compiler accepts it. Java Pure rejects this as an undeclared
multiplicity parameter.

**Repro / Context**

```pure
function <<PCT.test>> meta::pure::functions::math::tests::pow::testComplexPow<Z|h>(
    f:Function<{Function<{->Z[y]}>[1]->Z[y]}>[1]
):Boolean[1]
{
    assertEq(16.0, $f->eval(|2->pow(pow(2,2))));
    assertEqWithinTolerance(182.88729271224377725957310758531093597412109375,
                            $f->eval(|pow(3.33,4.33)), 0.0000000000001);
}
```

- Declared params: type `Z`, multiplicity `h`.
- Used in the signature: multiplicity `y` (twice, inside the nested
  `Function<{->Z[y]}>` type). `y` is **not** declared.
- Expected: compile error — "undeclared multiplicity parameter `y`".
- Actual: compiles clean.

**Notes**
- This function exists in the platform tree today (PCT pow tests). The
  fact that platform compile is "0 errors" means the validator is
  missing — not that the input is valid.
- Sweep the symmetric case: **undeclared type parameter** in the same
  position. A signature like `<|h>(f:Function<{->Q[h]}>[1])` (no `Q`
  declared) should also fail.
- Resolution sites to check (verify, don't trust): function signatures,
  qualified-property signatures, class type-parameter scopes, lambda
  parameter types, generic constraints.
- Suspect: when lowering `TypeRef`/`MultiplicityRef` inside a function
  signature, an unbound name is silently treated as a free
  `Generic(name)` / fresh multiplicity instead of being checked against
  the enclosing element's declared parameter list. Likely in
  `crates/pure/src/resolve.rs` or wherever signature type-args are
  bound (look for the path that turns `Z[y]` into a `Named` /
  `MultiplicityValue::Param`).
- Once the validator fires, the platform `.pure` source for this PCT
  test will need fixing — either rename `y → h` or declare `<Z|y,h>` /
  `<Z|y>`. That's a separate platform-source change; the compiler fix
  comes first so we see what else breaks.

<!-- agent-audit:start id=T-20260510-02 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
<!-- agent-audit:end -->

### T-20260510-01 — Import-less reference to another package resolves silently

- **Type:** parity-gap
- **Area:** compiler
- **Priority:** P1
- **Reporter:** Rafael
- **Filed:** 2026-05-10

**Summary**
A class can reference another class in a different package by short name
without importing that package, and the compiler accepts it. Java Pure
rejects this. The same hole appears on property type references.

**Repro / Context**
Sketch (Class2 lives in some package *other* than `abc`, with no
`import abc::*;`):

```pure
Class abc::Class1 {}

// expected: unresolved type error — `abc` is not imported
Class other::Class2 extends Class1 {
  // expected: unresolved type error on the property type too
  child : Class1[0..1];
}
```

- Expected: compile error (unresolved `Class1`) at both the `extends`
  clause and the property type.
- Actual: compiles clean.
- Java reference: `legend-pure-core` rejects short-name references that
  aren't reachable from the current section's imports (same package or
  explicit `import …::*;`). Confirm against `TestImport` / equivalent
  in the M3 parser tests.

**Notes**
- "Ensure the error does not occur elsewhere either" — sweep every
  short-name → element resolution site, not just `extends` and property
  types. Likely suspects (verify, don't trust the list): constraint
  expressions, qualified-property bodies, function parameter types,
  return types, `^Class(...)` constructors, `cast(@T)`, generic type
  arguments, stereotype/profile refs, association ends.
- Suspect: short-name resolution path doesn't filter candidates by
  visible-imports of the *use site's* section. Probably in
  `crates/pure/src/resolve.rs` (look for the package-walk that finds
  `Class1` regardless of imports).
- Sanity check that FQN references (`abc::Class1`) still work without
  an import — that's correct Java behaviour and must not regress.

<!-- agent-audit:start id=T-20260510-01 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
<!-- agent-audit:end -->

---

## Closed

<!-- Resolved / migrated / wontfix items, newest first. Keep the full block
     including the final audit-block status for posterity. -->

_(empty)_
