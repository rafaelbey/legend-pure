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

### T-20260513-01 — MCP / LSP: render_fqn returns mangled function names, blocking run_pct/run_test

- **Type:** bug
- **Area:** mcp | lsp | runtime
- **Priority:** P2
- **Reporter:** Rafael
- **Filed:** 2026-05-13

**Summary**
`list_tests` (MCP) and `code_lenses_for` (LSP) surface function FQNs in
their **mangled** form (e.g. `testPlus_Function_1__Boolean_1_`), but the
runner's `run_pct` / `run_test` pipe the FQN through Pure's
`pathToElement` native — which only accepts the **un-mangled** form
(`testPlus`). Result: an agent that calls `list_tests` and then `run_pct`
with the FQN it just got back hits `"pathToElement: path not found"`.

**Repro / Context**
- From the MCP smoke test on `legend-pure-rust` HEAD:
  ```
  list_tests prefix=meta::pure::functions::math::tests::plus
    → "fqn": "meta::pure::functions::math::tests::plus::testPlus_Function_1__Boolean_1_"
  run_pct test_fqn=…testPlus_Function_1__Boolean_1_  adapter_fqn=…InMemoryExecution_Function_1__X_o_
    → TOOL ERROR: "pathToElement failed: path not found: '…testPlus_Function_1__Boolean_1_'"
  ```
- Same shape in the IntelliJ ▶ Run gutter — when the lens dispatches
  `legend.runTest { args: ["<mangled>"] }`, the LSP-side
  `run_test_via_surveyor` calls `runTestsFromPath(<mangled>, "")` which
  internally uses `pathToElement`. The IDE flow happens to work today
  only because the gutter contributor strips the mangle suffix before
  emitting the command — that strip logic doesn't exist on the MCP
  side.

**Expected**
Either:
- (a) `render_fqn` (used by both `code_lenses_for` and MCP
  `list_tests` / `search_symbols`) returns the un-mangled form by
  default, with a separate `render_mangled_fqn` for the few places
  that need uniqueness across overloads, **or**
- (b) The runner accepts either form: try `pathToElement` with the
  input as-is, and if that errors with `path not found`, retry after
  stripping the `_Function_…__…_` suffix.

(a) is the structural fix and matches Java Pure's behaviour where
public FQNs never carry the dispatch mangle. (b) is a tactical
workaround acceptable as a stop-gap.

**Notes**
- `crates/runtime/src/runner/mod.rs` — `render_fqn` (private) used by
  `list_pct_adapters`.
- `crates/mcp/src/server.rs` — `render_fqn` (duplicate copy used by
  `search_symbols`, `read_element`, `list_tests`, `list_packages`).
- `crates/lsp/src/handlers.rs:692` — `render_fqn` (original copy).
  Three sites of the same logic — fixing once likely requires lifting
  to `legend_pure_runtime::query::render_fqn` or similar.
- IntelliJ run-gutter contributor: how it strips the mangle suffix
  is what we need to mirror in `runner::run_test` / `run_pct`. See
  `clients/intellij/src/main/kotlin/.../run/PureRunLineMarkerContributor.kt`.

---

### T-20260511-07 — LSP: Find Usages (workspace-wide references to an element)

- **Type:** feature
- **Area:** lsp
- **Priority:** P2
- **Reporter:** Rafael
- **Filed:** 2026-05-11

**Summary**
The LSP doesn't expose a "Find All References" / "Find Usages" command
for Pure elements. Right-clicking a class, function, or property and
asking "who references this?" returns nothing or only the definition.
Expected: every site across the workspace that refers to the symbol
under the cursor, navigable from the editor.

**Repro / Context**
- Put cursor on `abc::Class1` (declaration or any reference site) →
  invoke editor's "Find All References" → expect a list of every
  `.pure` location that:
  - references it by FQN (`abc::Class1`),
  - references it by short name under an `import abc::*;`,
  - extends it, implements it, or constructs it via `^abc::Class1(...)`,
  - uses it as a type argument (`List<abc::Class1>`),
  - uses it inside `cast(@abc::Class1)`,
  - references one of its properties / qualified properties (separate
    cursor target).
- Same query shape for functions (call sites + lambda captures) and
  properties (`.propA` access + `^Foo(propA = …)` binding sites).
- Acceptance criteria:
  - LSP `textDocument/references` returns the workspace-wide set.
  - Editor "go to references" / "find usages" surfaces every site.
  - Performance: indexed (subsecond for warm workspace), not a
    per-request whole-model scan.

**Notes**
- Index seam: `PureModel` already holds every chunk's `ExprKind`
  tree and every element's resolved `ElementId`. A side-index keyed
  by `ElementId → Vec<(chunk_id, source_info)>` populated during
  Pass 2b body-lowering covers function calls, property access,
  type refs, constructor receivers, generic args, cast targets,
  association ends, stereotype/profile refs.
- Adjacent to T-20260511-06 (global diagnostics): they share a
  dependency / reverse-reference index. Land the index once, expose
  it through two LSP commands.
- Distinct from `textDocument/definition` (already wired via
  `crates/lsp/src/handlers/*`) — that's forward; this is reverse.
- Not a parity gap with Java — Java has no LSP in this repo.

<!-- agent-audit:start id=T-20260511-07 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
<!-- agent-audit:end -->

### T-20260511-06 — Global diagnostics: surface every site impacted by a single change

- **Type:** feature
- **Area:** lsp | compiler
- **Priority:** P2
- **Reporter:** Rafael
- **Filed:** 2026-05-11

**Summary**
When a user edits one element (rename, signature change, property
type/multiplicity change, supertype change, etc.), there's no global
view of the downstream call/reference sites that the edit invalidates.
Today diagnostics surface only for files the editor has opened — the
"what else just broke?" answer requires a full workspace rebuild.

**Repro / Context**
- Use case: rename `abc::Class1` → `abc::Foo`, or flip a property's
  multiplicity from `[1]` to `[0..1]`, or narrow a function's return
  type. Expect: a single panel listing every `.pure` source location
  in the workspace whose compile now fails because of that edit.
- Acceptance criteria (sketch — refine at triage):
  - LSP publishes diagnostics for every chunk that imports or
    transitively depends on the changed element, not just the edited
    file.
  - Cross-chunk validators (`validate_repo_visibility`,
    `validate_constructor_bindings`, `validate_format_specifiers` if
    re-added, etc.) already run model-wide; the missing piece is
    surfacing their per-file output as a workspace-level panel.
  - Performance: incremental — re-validate only chunks whose
    dependency set touches the edited element's ElementId; don't
    redo the whole workspace on every keystroke.

**Notes**
- Adjacent existing infra to lean on:
  - `crates/lsp/src/diagnostics.rs` — diagnostic conversion already
    in place; this is about *publishing scope*, not message shape.
  - `crates/pure/src/validate.rs::validate(model)` — already walks
    every chunk and produces `CompilationError`s with `source_info`
    pointing at the offending site. The data is there; the LSP just
    needs to publish across files.
  - `crates/pure/src/model.rs` — ElementId → chunk mapping; suitable
    for "which chunks reference this id?" dependency index.
- Open design questions for triage:
  - Where the dependency index lives (in `PureModel`, in a separate
    crate, lazy vs. eager build).
  - What invalidation granularity is right (per-ElementId, per-chunk,
    per-section).
  - Whether to expose a CLI sibling (`legend check --impacted-by
    abc::Class1`) so non-LSP users get the same answer.
- Not a parity gap with Java — Java's IDE story is similarly
  per-file. This is a UX win specific to the Rust port.

<!-- agent-audit:start id=T-20260511-06 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
<!-- agent-audit:end -->

---

## Closed

<!-- Resolved / migrated / wontfix items, newest first. Keep the full block
     including the final audit-block status for posterity. -->

### T-20260511-05 — `format(...)` doesn't compile-time-check `%`-specifiers against arg types

- **Type:** parity-gap
- **Area:** compiler
- **Priority:** P2
- **Reporter:** Rafael
- **Filed:** 2026-05-11

**Summary**
`'…'->format([...])` accepts a list whose element types don't match
the format string's `%`-specifiers. Java Pure validates this at compile
time when the format string is a literal and the args list is a literal
collection.

**Repro / Context**

```pure
function abc::demo(): String[1] {
    '%s %d %d'->format(['hello', 'world', 4])
}
```

- Expected: compile error — position 2 (`%d`) expects Integer but got
  String (`'world'`).
- Actual: compiles and runs (runtime silently coerces / accepts).

**Notes**
- Java reference: format-string validation in
  `m3.compiler.validation.functionExpression.format` (or equivalent
  native-specific validator). Java's check runs only when both the
  format string and the arg list are statically determinable.
- Specifier matrix (confirm against Java at fix time):
  `%s`=Any (no type constraint), `%d`/`%i`=Integer, `%f`/`%e`=Float
  (or Number), `%t`=DateTime, `%r`=Number (raw), `%b`=Boolean,
  `%%`=literal percent.
- Two axes per specifier: (a) **arity** — count of `%`-specifiers
  (excluding `%%`) must equal list length; (b) **per-position type**
  — list element at position N is subtype-compatible with the
  declared spec at position N. Multiplicity is always `[1]` per
  element of the list — no multiplicity axis beyond arity.
- Validator placement: post-inference, native-specific validator on
  `format`'s `ElementId`. Most natural seam is a cross-chunk pass
  in `validate(model)` (same family as the constructor-binding
  validators), so it sees fully-inferred arg types. Skip silently
  when either the receiver isn't a literal `String` or the arg list
  isn't a literal `Collection` (`ExprKind::Collection` of literals /
  inferable expressions) — runtime is the fallback for dynamic cases.
- Adjacent natives worth sweeping for the same hole:
  - `print` / `println` if they accept a format-style arg,
  - other string-template helpers (`replace`-style templating, URL
    formatters in DSLs).
- This is parity-gap territory: a Pure user porting from Java will
  expect the same compile-time signal. Today the bug is silently
  swallowed at runtime (the `%d` formatter on a `String` coerces to
  string and emits the wrong shape).
- Error shape: new `CompilationErrorKind::FormatSpecifierTypeMismatch`
  variant with `{ specifier: SmolStr, position: usize, expected_type:
  SmolStr, got_type: SmolStr }`, plus a sibling
  `FormatSpecifierArityMismatch { specifiers: usize, args: usize }`.
  Confirm against Java's diagnostic shape before finalizing the
  variants.

<!-- agent-audit:start id=T-20260511-05 -->
- 2026-05-11 — claimed by claude-opus-4-7[1m] — investigating the
  format native at `crates/runtime/src/native/string.rs` plus the
  Java reference upstream. Status: Investigating
- 2026-05-11 — diagnosis correction: the TODO Notes block claimed
  Java validates `format` at compile time
  (`m3.compiler.validation.functionExpression.format`). That class
  does not exist — verified by walking
  `legend-pure-core/legend-pure-m3-core/src/main/java/.../compiler/
  validation/`. Java validates format **only at runtime** in
  `legend-pure-runtime-java-engine-interpreted/.../Format.java` and
  `legend-pure-runtime-java-engine-compiled/.../PureStringFormat.java`
  (throws `IllegalArgumentException("Expected <T>, got: <v>")`).
  The actual parity gap is at the Rust runtime, NOT at compile time:
  * Rust `%d` silently coerced via `as_integer().unwrap_or(0)`
    (`string.rs:602`).
  * Rust `%f` fell through to `f64::NAN` for non-Number types and
    auto-coerced Integer/Decimal — Java rejects all of those
    (`Format.java:156` — `instanceOf(arg, M3Paths.Float)`).
  * Rust `%t` silently fell through to `pure_to_string` for non-Date
    args.
  Specifier matrix correction: Java has no `%i`, `%e`, or `%b` — the
  TODO Notes listed shapes that don't exist. Real set: `%s`, `%r`
  (both Any), `%d` (Integer), `%f` (Float strict), `%t` (Date with
  subtype OK), `%%` literal.
- 2026-05-11 — fix landed (runtime parity, commit `d4326dc8291`):
  tightened `%d`/`%f`/`%t` arms in `Format::execute`
  (`crates/runtime/src/native/string.rs`) to throw
  `"Expected <T>, got: <v>"`, matching Java's exact message text
  from `Format.java:107/117/158`. 7 new runtime tests + 3 positive-
  control regressions. Workspace + `builds_platform_purem_and_round_trips`
  clean — no shipped Pure source relied on the silent coercion.
- 2026-05-11 — compile-time validator attempted then reverted under
  strict-Java-parity policy:
  * Briefly added a compile-time `validate_format_specifiers`
    cross-chunk pass with two new `CompilationErrorKind` variants
    plus shared `crates/pure/src/format_spec.rs` parser. Initially
    scoped under user-approved "Both runtime parity + compile-time
    validator" option.
  * Caught the platform's own runtime-error PCT tests at compile
    time (`testFormatTooFewInputs`, `testFormatTooManyInputs` in
    `format.pure:131-137`). The intermediate fix tweaked those tests
    to use a `let`-bound format string so the static check would
    defer to runtime — but that introduced the awkwardness of
    modifying platform `.pure` source to evade our own check.
  * User reconsidered: "i dont like the changes to format.pure.
    Seems our port does not behave like java. If so why?". Chose
    "Revert the compile-time validator (strict Java parity)" via
    clarification prompt — the runtime parity fix already matches
    Java's runtime behavior exactly, and Java has no compile-time
    validator to mirror.
  * Reverted in commit (TBD — this revert + audit commit). All
    validator code, error variants, LSP/categorize arms, validator
    tests, shared format-spec parser, and the `format.pure` let-
    binding tweak are gone. Workspace ends in strict Java parity.
- 2026-05-11 — verification (post-revert):
  * `cargo build --workspace` clean.
  * 10 runtime format tests (3 pre-existing + 7 added in
    `d4326dc8291`) green.
  * `builds_platform_purem_and_round_trips` clean.
  Status: Fix landed (runtime parity: commit d4326dc8291).
- 2026-05-11 — follow-ups (audit notes):
  * If a future change adds a compile-time format validator, the
    PCT runtime-error tests at `format.pure:131-137` need a
    test-pattern that doesn't trip the static check (e.g. lambda-
    deferred / non-literal args). Don't repeat the platform-edit
    workaround used in the reverted attempt.
  * Adjacent natives (`print` / `println`, URL formatters, DSL
    string-template helpers) — none currently use format-style
    specifiers; revisit if they're added.
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
- 2026-05-11 — claimed by claude-opus-4-7[1m] — reproducing locally,
  then auditing short-name resolution sites in `crates/pure/src/resolve.rs`.
  Status: Investigating
- 2026-05-11 — diagnosis revised after reading the resolver:
  `resolve.rs`'s `resolve_unqualified` (L604-707) is sound — its step 2
  import-scope scan is correctly gated on the scopes it's given, and
  step 3's root-only fallback finds only root M3 aliases
  (`resolve_by_path` is non-recursive — `model.rs:599-623`). The leak
  is upstream in **`pipeline.rs`**:
  * Pass 2a `pipeline.rs:1124-1132` and Pass 2b `:1213-1221` mutated
    the section's cached `import_scopes` Vec in place with each
    element's implicit self-package. When element A (in `abc::`) was
    processed first, `abc::` got pushed; when element B (in `other::`)
    was processed second, the cache contained BOTH `abc::` AND
    `other::`. B's short-name lookup then resolved against the leaked
    A-package entry.
  * Secondary bug: `resolve_caches.entry(scope_key)` at
    `pipeline.rs:1135 / :1222` keyed cached resolutions by section, so
    element A's `name → ElementId` result also bled into element B's
    identical-name lookup. Removing only the scope mutation without
    scoping the cache per-element would have left the same leak.
- 2026-05-11 — implementation:
  * `crates/pure/src/resolve.rs`: new
    `self_package: Option<&'a Package>` field on `ResolutionContext`
    (mirrors `type_parameters` / `multiplicity_parameters`); new step
    2b in `resolve_unqualified` and in `resolve_function_call`'s
    unqualified branch — consulted only when explicit imports yielded
    nothing, so explicit imports keep shadowing same-package siblings
    (Java parity).
  * `crates/pure/src/lower/function_app.rs`: same step 2b fallback in
    `candidates_by_arity`, the lambda-inference helper that walks
    `import_scopes` directly. Without this, parametric recursive fns
    in the same package lost their lambda-expected-type derivation
    (surfaced as a regression in
    `tic_recursive_generic_fn_no_unbound_param_error`).
  * `crates/pure/src/pipeline.rs`: deleted both in-place
    `import_scopes.push(...)` mutation blocks; converted
    `resolve_caches` from section-keyed `HashMap` to per-element local
    `HashMap` (sharing across elements is unsound because results
    depend on `self_package`); dropped the parameter from
    `pass_define_bodies` / `pass_define_class_bodies` and the third
    element from `pass_define_signatures`'s return tuple.
  * `crates/pure/src/extension.rs`: `self_package: None` for
    free-floating DSL expressions — `auto_imports` already covers the
    visible namespace.
  * `crates/pure/tests/import_isolation_smoke.rs` (new) — 8 tests:
    bug repro (extends + property), with-import regression, FQN
    regression, same-section two-packages leak, function-body
    cross-pkg call, two-sections isolation, element-order
    independence, same-package short-name regression.
- 2026-05-11 — verification:
  * `cargo build --workspace` green.
  * 8 new import-isolation tests + 26 existing `inference_context`
    tests + per-crate sweep across pure, snapshot-builder,
    core-platform, lsp, dsl-mapping, dsl-relational, dsl-diagram,
    runtime, dsl-tds, dsl-store, dsl-graph, dsl-mapping-runtime,
    dsl-relational-runtime — all green.
  * `builds_platform_purem_and_round_trips` green — the platform's
    real `.pure` corpus is import-discipline clean, no shipped source
    relied on the leak.
  * `cargo fmt --check` clean on touched files. `cargo lint-lib` clean
    (2 pre-existing warnings in `dsl-mapping` are unrelated).
    Copyright clean (441 files).
  Status: Fix landed (commit 1c47266dda8).
- 2026-05-11 — follow-ups (audit notes, not separate TODOs cut yet):
  * The TODO's original Notes block fingered `resolve.rs` as the
    suspect; the actual leak was in `pipeline.rs`. Worth flagging so
    future readers don't repeat the wrong-suspect path.
  * Three short-name walkers now share the self-package fallback
    (`resolve_unqualified`, `resolve_function_call`,
    `candidates_by_arity`). If a fourth appears, it needs the same
    pattern — grep `ctx.import_scopes` to audit.
  * Java's specific-element-import form `import abc::Class1;` (vs. the
    wildcard `import abc::*;` supported today) is orthogonal to this
    bug and deferred — the parser may or may not already accept the
    syntax; semantic plumbing is an `ImportScope` change.
- 2026-05-11 — sweep follow-up: the original TODO note called out
  "sweep every short-name → element resolution site, not just `extends`
  and property types" with a hopeful (verify-don't-trust) list. The
  structural fix lives at `resolve_unqualified` in
  `crates/pure/src/resolve.rs` — every site below provably routes there
  via `resolve_type_ref` (L167) or `resolve_element_ptr` (L969). Added 8
  new tests to `crates/pure/tests/import_isolation_smoke.rs` (now 16
  total) pinning the additional sites that the original 8 tests didn't
  explicitly cover:
  * function parameter type (no-import errors + with-import compiles),
  * function return type,
  * `^Class(...)` constructor receiver class FQN,
  * `cast(@T)` type argument,
  * Association end class type,
  * Stereotype profile reference,
  * Generic type argument inside `<…>` (uses a local generic `Class
    lib::Box<T>` to stay self-contained — first attempt referenced
    `meta::pure::functions::collection::List` which isn't in the test
    bootstrap and produced a false-negative).
  Workspace nextest: 1965/1965 (+16 over the 1949 baseline; all new
  sweep tests green).
  Status: Fix landed + sweep coverage confirmed (no additional fix
  needed — central chokepoint covers every site).
<!-- agent-audit:end -->


### T-20260511-04 — `^Class(unknownProp = value)` doesn't error on properties that don't exist

- **Type:** parity-gap
- **Area:** compiler
- **Priority:** P1
- **Reporter:** Rafael
- **Filed:** 2026-05-11

**Summary**
A `^Class(...)` instance-construction `KeyExpression` doesn't check
that the property name actually exists on the class (or any
supertype). Supplying an undeclared property name is silently
accepted. Java Pure rejects this.

**Repro / Context**

```pure
Class abc::Class1
{
  propA : Integer[1];
}

// Expected: compile error — `propd` is not a property of `abc::Class1`.
^abc::Class1(propd = '');
```

- Expected: compile error along the lines of
  `"Class 'abc::Class1' has no property 'propd'"` (mirrors the
  existing `UnknownProperty` diagnostic that fires on
  `$x.propd` access — same idea, different binding site).
- Actual: compiles clean.

**Notes**
- Same lowering seam as T-20260511-02 and T-20260511-03: the
  `KeyExpression` resolver in `^Class(...)` lowering. Where supplied
  keys are matched against the class's property list, an unmatched
  key must produce `CompilationErrorKind::UnknownProperty` (reuse
  the existing variant — same shape, same consumer expectations).
- Sweep:
  (a) supertype-walked: `propA` is fine when declared on a parent
      `Class1` extends `Base { propA: …; }`, only fail when truly
      absent across the whole supertype chain,
  (b) reserved-name collision (e.g. `^Class(class = …)` — class is
      a reserved keyword, should fail at parse, not here),
  (c) typo near a real property name — the message should be plain
      "no property X"; suggestion is nice-to-have but not required
      for parity (Java's diagnostic is plain).
- The four property-binding-site validators (T-20260511-01..04)
  almost certainly share one walk: lower `^Class(...)`, resolve
  receiver class + properties (with supertypes), for each supplied
  key: (i) does it exist? (T-20260511-04) (ii) is the value
  compatible with declared type+mult? (T-20260511-03); after all
  keys: (iii) are all required keys supplied? (T-20260511-02).
  T-20260511-01 (default-value type) is a separate site (class body,
  not constructor) but uses the same compat helper.
- The existing runtime-level "UnknownProperty" diagnostic kind
  (`CompilationErrorKind::UnknownProperty { type_name, property_name }`
  per workspace memory) is the right variant to emit. No new error
  kind needed.

<!-- agent-audit:start id=T-20260511-04 -->
- 2026-05-11 — claimed by claude-opus-4-7[1m] — bundled with
  T-20260511-01/02/03 as one constructor-binding family fix.
  Status: Fix landed (commit c5b4d90a120). See unified
  details under T-20260511-01's audit block.
<!-- agent-audit:end -->

### T-20260511-03 — `^Class(prop = value)` doesn't type-check or multiplicity-check the supplied value

- **Type:** parity-gap
- **Area:** compiler
- **Priority:** P1
- **Reporter:** Rafael
- **Filed:** 2026-05-11

**Summary**
A `^Class(...)` instance-construction `KeyExpression` doesn't check
that the supplied value's inferred type and multiplicity are compatible
with the property's declared type and multiplicity. Wrong-type and
wrong-multiplicity assignments are silently accepted. Java Pure rejects
this.

**Repro / Context**

```pure
Class abc::Class1
{
  propA : Integer[1];
}

^abc::Class1(propA = '');     // String on Integer — should fail
^abc::Class1(propA = []);     // 0 on [1]          — should fail
^abc::Class1(propA = [1, 2]); // 2 on [1]          — should fail
```

- Expected: compile error at each call. Type mismatch (case 1),
  multiplicity-too-low (case 2: `[0]` doesn't satisfy `[1]`),
  multiplicity-too-high (case 3: `[2]` doesn't satisfy `[1]`).
- Actual: all three compile clean.

**Notes**
- Three distinct axes per `KeyExpression`, mirrors the function-call
  argument-binding rules:
  - **Type:** supplied expr's inferred element type is a subtype of
    the property's declared type. Subtype OK
    (`Integer` accepted for `Number[1]`), unrelated type fails.
  - **Multiplicity lower bound:** supplied multiplicity's lower bound
    `>=` property's lower bound. `[0]` (empty list literal) doesn't
    satisfy `[1]` or `[1..*]`. Same rule as function arg binding.
  - **Multiplicity upper bound:** supplied multiplicity's upper bound
    `<=` property's upper bound. `[2]` doesn't satisfy `[1]` or
    `[0..1]`.
- The compat function to call is the same canonical one referenced
  in T-20260511-01 and T-20260510-03 (FunctionType binding): likely
  `is_type_compatible_structural` for type, plus a sibling
  multiplicity-compat helper. Verify the actual name in
  `crates/pure/src/infer.rs`. Reuse — don't re-implement.
- Same lowering seam as T-20260511-02 (constructor `KeyExpression`
  resolution); the three bugs share a binding-site walk: for each
  supplied key, resolve the property by name, then validate
  (type + mult). Required-property check (T-20260511-02) runs after
  all keys are bound and diffs supplied-set against required-set on
  the class.
- Sweep adjacent cases that should also fail / pass correctly:
  (a) `propA = $someVar` where `$someVar` is `String[1]` — fail,
  (b) `propA = if(...){1}else{''}` — fail (joined return type is
      `Any[1]` or similar, not `Integer`),
  (c) `propA = 1` — pass,
  (d) `propA = $intMaybe` where `$intMaybe: Integer[0..1]` against
      a `[1]` slot — fail on multiplicity even though type is right,
  (e) inherited properties supplied via `^Subclass(superProp = …)`
      validated against the supertype's declared shape.
- Error shape: parity with Java's `NewInstance` / property-binding
  validator. Likely two distinct kinds (type vs. multiplicity) or
  one combined kind with a discriminator — match what Java emits.
  Cite the Java class + message string in the audit block at fix
  time.
- This is the third entry in the property-binding-site family
  (T-20260511-01: default-value type, T-20260511-02: missing
  required, T-20260511-03: supplied-value type/multiplicity).
  Worth keeping in mind that all three may share infrastructure:
  the "given a property and a value, are they compatible?" helper
  is the same regardless of whether the value comes from a `=` in
  the class body or a `KeyExpression` in `^Class(...)`.

<!-- agent-audit:start id=T-20260511-03 -->
- 2026-05-11 — claimed by claude-opus-4-7[1m] — bundled with
  T-20260511-01/02/04 as one constructor-binding family fix.
  Status: Fix landed (commit c5b4d90a120). See unified
  details under T-20260511-01's audit block.
<!-- agent-audit:end -->

### T-20260511-02 — `^Class(...)` constructor doesn't check required properties are set

- **Type:** parity-gap
- **Area:** compiler
- **Priority:** P1
- **Reporter:** Rafael
- **Filed:** 2026-05-11

**Summary**
A `^Class(...)` instance-construction expression doesn't verify that
every required property (multiplicity lower bound `>= 1` with no
default value) is supplied via a `KeyExpression`. Constructing a class
with required slots left empty is silently accepted. Java Pure rejects
this.

**Repro / Context**

```pure
Class abc::Class1
{
  propA : Integer[1];
}

// Expected: compile error — required property `propA` not supplied
// and has no default value.
^abc::Class1();
```

- Expected: compile error along the lines of `"Missing required property
  'propA' on class 'abc::Class1' in constructor"` (confirm against Java's
  shape — likely emitted from the same validator that checks property
  arity at constructor sites).
- Actual: compiles clean.

**Notes**
- Required = `multiplicity.lower_bound >= 1` AND no `default_value` on
  the `Property` shell. A `[0..1]` / `[*]` / `[0..*]` property is
  optional; `[1]` / `[1..*]` with no default is required.
- Default-value satisfies the check: `propA : Integer[1] = 0;` should
  make the empty-args constructor compile cleanly. (Once
  T-20260511-01 lands, the default itself is type-checked — same
  property shell, two adjacent validators.)
- Sweep adjacent cases — all should produce a compile error:
  (a) `^Class()` with required prop missing entirely,
  (b) `^Class(otherProp = 1)` where the missing prop is required,
  (c) inherited required props from a supertype that the constructor
      doesn't supply (walk `super_types` chain, union the required
      sets).
- Constructor expression lowering happens in
  `crates/pure/src/lowering/` (or wherever `^Class(...)` →
  `ExprKind::*` happens — verify, don't trust). The check belongs
  right after the keys are bound: at that point we know
  (i) the receiver `Class`, (ii) the slice of supplied `KeyExpression`
  names, (iii) the class's resolved properties (already on the
  shell from Pass 1, with supertypes joinable via `super_types`).
- Inherited-property walk should follow the same trail as the
  `KeyExpression` resolver — wherever short-name keys are looked up
  against the class's full property list, that's the list to diff
  against required-set.
- Error shape: parity with Java's diagnostic. The Java validator is
  somewhere under `m3.compiler.validation.validator.*` — likely
  `ClassValidator` or a `NewInstance` checker. Cite path + class
  name in the audit block at fix time.
- Same family as T-20260511-01 (property default value type check):
  both add property-binding-site validators. Could share a small
  helper for "what does this property require?".

<!-- agent-audit:start id=T-20260511-02 -->
- 2026-05-11 — claimed by claude-opus-4-7[1m] — bundled with
  T-20260511-01/03/04 as one constructor-binding family fix.
  Status: Fix landed (commit c5b4d90a120). See unified
  details under T-20260511-01's audit block.
<!-- agent-audit:end -->

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
- 2026-05-11 — claimed by claude-opus-4-7[1m] — bundled with
  T-20260511-02/03/04 as one constructor-binding family fix
  (per the inbox notes: all four share a "given a property and a
  value, are they compatible?" core operation).
- 2026-05-11 — design (after Phase 1 exploration):
  * Original plan: T-04 + T-02 eager in `lower_new_instance`,
    T-01 + T-03 cross-chunk. Inputs to T-04 and T-02 are all known
    syntactically.
  * Empirical correction: `lower_new_instance` runs in Pass 2b BEFORE
    `rebuild_derived_indexes()` (pipeline.rs:361, end of
    `compile_repo_slice`), so association-injected ends aren't
    queryable yet. All four must run cross-chunk in `validate(model)`.
  * Java parity: T-02's missing-required check excludes
    association-injected properties — Java's `NewInstance` validator
    treats them as bidirectional runtime links, not constructor
    inputs. Helpers split: `find_property_full_with_inheritance`
    walks supertypes + injected (for T-03/T-04 key resolution);
    `all_declared_properties_with_inheritance` walks supertypes only
    (for T-02 required-set).
- 2026-05-11 — implementation:
  * `crates/pure/src/resolve.rs`: new `find_property_full_with_inheritance`,
    `all_declared_properties_with_inheritance`; `mult_bounds` promoted
    to `pub(crate)`.
  * `crates/pure/src/infer.rs`: `render_type` promoted to `pub(crate)`.
  * `crates/pure/src/error.rs`: three new variants —
    `PropertyDefaultValueIncompatible`,
    `ConstructorMissingRequiredProperty`,
    `ConstructorPropertyTypeMismatch`.
    T-04 reuses the existing `UnknownProperty` variant.
  * `crates/pure/src/validate.rs`: new `validate_property_default_values`
    (T-01) and `validate_constructor_bindings` (T-02 + T-03 + T-04),
    both wired into `validate(model)` cross-chunk. New private
    walker `visit_value_specs_in_element` recurses every ExprKind to
    find every `FunctionCall("new", …)` regardless of nesting depth.
  * `crates/pure/src/pipeline.rs::pass_infer`: extended with new
    `TargetKind::ClassPropertyDefault` / `AssociationPropertyDefault`
    arms so property default-values get their `type_info` populated
    (latent gap surfaced while implementing T-01 — defaults were
    lowered but never inferred).
  * `crates/lsp/src/diagnostics.rs` +
    `crates/core-platform-pure/tests/categorize_errors.rs`:
    exhaustive-match arms for the three new error kinds.
  * `crates/pure/tests/property_default_value_smoke.rs` — 5 tests
    (T-01).
  * `crates/pure/tests/constructor_binding_smoke.rs` — 9 tests
    (T-02 / T-03 / T-04).
- 2026-05-11 — verification:
  * `cargo build --workspace` green.
  * 14 new smoke tests green.
  * `cargo test -p legend-pure-parser-pure -p legend-pure-snapshot-builder
    -p legend-pure-core-platform -p legend-pure-lsp -p legend-pure-dsl-mapping
    -p legend-pure-dsl-relational -p legend-pure-dsl-diagram` green
    (incl. doctests).
  * `cargo test -p legend-pure-snapshot-builder
    builds_platform_purem_and_round_trips` green — the platform's real
    `^Class(...)` constructor sites all satisfy the new validators
    (implicit platform sweep).
  * `cargo fmt --check` clean on touched files.
  * `cargo clippy --lib -p legend-pure-parser-pure -- -D unwrap_used
    -D expect_used` clean.
  * `./scripts/check-copyright.sh` clean (436 files).
  Status: Fix landed (commit c5b4d90a120).
- 2026-05-11 — `+=` (augmented) constructor bindings handled with the
  same compat rule as `=` for v1. Correct for `[*]`/`[1..*]` slots,
  conservative for `[1]`/`[0..1]`. Follow-up: file a TODO for the
  v2 "element-type compat against collection slot's element type"
  rule.
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
  Status: Fix landed (commit f1a54784a6a).
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
- 2026-05-11 — claimed by claude-opus-4-7[1m] — picked from top of Open.
- 2026-05-11 — root cause: `crates/pure/src/infer.rs:976-978` had a
  3-line `if matches!(param_te, TypeExpr::FunctionType { .. }) { continue; }`
  bypass that ran *immediately before* the structural compat call at
  L988. `is_type_compatible_structural` (`resolve.rs:1946`) already
  handles FunctionType correctly — its arm at L1989-2012 recurses
  through arity + per-position types + return type. The skip was
  load-bearing dead weight that hid every FunctionType-param
  mismatch. Predates the structural-compat work (the earlier nominal
  `is_type_compatible` indeed couldn't handle FunctionType, so
  v1-of-this-validator was forced to skip).
- 2026-05-11 — secondary gap inside `is_type_compatible_structural`'s
  FunctionType arm: parameter tuple iteration dropped the
  `Multiplicity` half (`for ((a_te, _), (p_te, _)) in …`) and the
  return side never compared `return_multiplicity`. Sweep case (b)
  from the TODO note (wrong parameter multiplicity) was unreachable.
- 2026-05-11 — implementation:
  * `crates/pure/src/infer.rs`: deleted the L976-978 bypass.
  * `crates/pure/src/resolve.rs`: extended the FunctionType arm of
    `is_type_compatible_structural` — destructure binds
    `return_multiplicity` on both sides; per-position now also calls
    `is_multiplicity_compatible(Some(a_mult), p_mult)`; trailing
    return-side check also calls multiplicity compat. Variance stays
    covariant-uniform (Java's contravariance is a separate semantic
    change; deferred).
  * `crates/pure/src/infer.rs::render_type_expr`: replaced the
    `<FunctionType>` stub with a Pure-syntax renderer
    (`Function<{T[m]->V[n]}>`), reusing the existing private
    `render_multiplicity` helper. Diagnostic-message fallbacks at
    L989/L995 switched from `element_name` / `format!("{:?}")` to
    `render_type` so lambda args + FunctionType params surface
    symmetrically (no more `got <unknown>` on lambda mismatches).
  * `crates/pure/tests/integration_tests.rs`: 5 new tests covering
    each axis — arity (1-arg for 0-arg, 2-arg for 1-arg), param type,
    return type, param multiplicity, subtype-OK positive control.
- 2026-05-11 — verification:
  * `cargo build --workspace` green.
  * 5 new + 6 pre-existing FunctionType tests in `integration_tests.rs`
    all green; `function_type_higher_order_wrong_inner_type_errors`
    still passes (now via a second error path — the structural-compat
    diagnostic fires alongside the binding-inference + return-type
    paths that existed before).
  * Per-crate sweep: pure, snapshot-builder, core-platform-pure,
    dsl-mapping, dsl-relational — all green.
  * Platform end-to-end (`builds_platform_purem_and_round_trips`)
    green — no shipped `.pure` source had latent FunctionType
    arity/mult violations.
  * `cargo fmt --check` clean. Copyright clean (440 files).
    `cargo clippy --lib -- -D unwrap_used -D expect_used` clean
    (the 2 warnings remaining are pre-existing).
  Status: Fix landed (commit 8dd5df52ccb).
- 2026-05-11 — follow-ups filed in notes (no separate TODOs cut yet):
  * Variance: covariant-uniform for now; Java's contravariant params /
    covariant return rule is a separate design pass.
  * `narrow_candidates_by_type` (`resolve.rs:3574`) still calls
    `is_type_compatible` (nominal) for narrowing — dispatch candidate
    filter doesn't yet narrow on FunctionType shape; only
    `validate_call_arguments` rejects post-pick.
  * `CompilationErrorKind::UnresolvedElement { path: "argument-type-mismatch:…" }`
    is a sentinel-string overload — fine for now, dedicated
    `ArgumentTypeMismatch` kind worth adding once tooling needs it.
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
- 2026-05-11 — claimed by claude-opus-4-7[1m] — picked from top of Open.
- 2026-05-11 — root cause: three co-located gaps in `crates/pure/src/resolve.rs`.
  (a) `resolve_type_ref`'s `MultiplicityArgument::Identifier` arm at L216
  created `Multiplicity::Variable(name)` unconditionally.
  (b) `lower_multiplicity` (L664) — context-free transformer used by
  nine signature-side call sites — did the same for
  `ast_type::Multiplicity::Variable(name)`.
  (c) `ResolutionContext` (L102) carried `type_parameters` but no
  sibling for multiplicity parameters. Plus a related downstream bug:
  `resolve_function_type_sentinel` (L298) silently collapsed
  `MultiplicityArgument::Identifier` to `Multiplicity::ZeroOrMany`
  (L327/L345), losing the name entirely.
- 2026-05-11 — implementation:
  * `crates/pure/src/resolve.rs`: new `multiplicity_parameters` field
    on `ResolutionContext`; new `resolve_multiplicity_with_validation`
    helper; wired into nine signature-side call sites; `Identifier`
    arm in mult-arg loop also routes through the helper; sentinel
    decoder preserves `Variable` and validates it instead of
    collapsing.
  * `crates/pure/src/pipeline.rs`: new
    `ast_multiplicity_parameters(element)` mirroring `ast_type_parameters`;
    threaded into every ctx construction site (3 in pipeline + 1 in
    extension + 2 in test fixtures).
  * `crates/pure/src/error.rs`: new
    `CompilationErrorKind::UndeclaredMultiplicityParameter { parameter }`
    variant. Distinct from `UnresolvedMultiplicityParameter` (which
    is post-dispatch at call sites).
  * Exhaustive-match arms in `crates/lsp/src/diagnostics.rs` and
    `crates/core-platform-pure/tests/categorize_errors.rs`.
  * 5 new tests in `crates/pure/tests/integration_tests.rs`:
    - Undeclared mult inside nested FunctionType (user's repro).
    - Declared mult (regression).
    - Undeclared top-level param mult.
    - Undeclared class-property mult.
    - Undeclared return-type mult.
- 2026-05-11 — platform sweep:
  surfaced one latent violation in
  `legend-pure-core/.../essential/lang/cast/toMultiplicity.pure:17`,
  where `<T|m>` declared `m` but the body used `z` twice. Fixed
  in the same commit by renaming the declared parameter to `z`
  (the natural choice — `m` was never used).
- 2026-05-11 — verification:
  * `cargo build --workspace` green.
  * 5 new + 12 existing FunctionType tests in `integration_tests.rs`
    all green; existing
    `function_type_higher_order_wrong_inner_type_errors` continues
    to pass.
  * Per-crate sweep across pure, snapshot-builder, core-platform-pure,
    lsp, dsl-mapping, dsl-relational — all green.
  * `builds_platform_purem_and_round_trips` green — implicit platform
    sweep across every shipped repo.
  * `cargo fmt --check` clean. Copyright clean (440 files). Clippy
    `lint-lib` clean.
  Status: Fix landed (commit 73abe40d0d6).
- 2026-05-11 — follow-ups (audit notes, not separate TODOs cut yet):
  * Undeclared TYPE-parameter sweep (the TODO's symmetric case) —
    deferred. `resolve_type_ref:191` partially covers it; the
    remaining gap (single-letter name that happens to resolve to a
    real class) is a semantic question.
  * Relation-column multiplicity validation
    (`resolve.rs:506`-area) — not signature-side; not in bug scope.
  * Parser-level fix for `parse_function_type_as_type_ref` wrapping
    return-side `Variable` in `Concrete` — addressed downstream via
    the validating lower's "accept both wrapper shapes" pattern,
    but a parser-side correction would be cleaner.
<!-- agent-audit:end -->

