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

### T-20260513-02 — IntelliJ plugin should restart the LSP subprocess on classpath / library config change

- **Type:** feature
- **Area:** clients-intellij
- **Priority:** P2
- **Reporter:** Rafael
- **Filed:** 2026-05-13

**Summary**
The `legend lsp` server reads its classpath / library configuration
(canonically `legend-pure-classpath.toml`) once at startup and never
re-reads it. When the user edits the classpath descriptor — adding a
filesystem repo, swapping a `.purem` artifact, changing a library
version — the server keeps serving against the stale model until the
user manually triggers "Restart Language Server" in the IDE. The
**IntelliJ plugin** already owns the server's process lifecycle via
the Platform LSP API, so the right place to react to a config change
is the client, not the Rust server. Watch the relevant file(s) from
the plugin and call the restart hook on the Legend Pure
`LspServerDescriptor` when they change.

**Repro / Context**
- Open an IntelliJ project that resolves `legend-pure-classpath.toml`
  (either via the configured setting in
  `clients/intellij/.../LegendPureConfigurable.kt:62` or the
  server-side ancestor walk).
- With the editor open, edit the descriptor to add a new filesystem
  repo or change an existing `kind`/path.
- Observe: diagnostics, hover, completion, and go-to-def all continue
  to reflect the pre-edit classpath. The new repo's elements are
  invisible until the user manually invokes "Restart Language Server".
- Acceptance criteria:
  - The plugin watches the resolved classpath descriptor (the path
    set in `LegendPureSettings`, falling back to ancestor-walking
    `legend-pure-classpath.toml` from the project root) for
    `modify`/`replace`/`delete` events.
  - On a detected change: the plugin invokes the IntelliJ Platform
    LSP API's restart entry point for the Legend Pure server, which
    tears down the subprocess and respawns it. IntelliJ re-issues
    `didOpen` for every editor that was open, so buffer state is
    preserved by the platform.
  - Debounce: rapid successive writes (editor autosave, atomic
    rename, `mv`) coalesce into a single restart (~500 ms window).
  - User-visible signal: a status-bar notification or balloon
    explaining the restart so the user understands why diagnostics
    blinked. Coordinate with T-20260512-02 (status-bar widget) if
    that lands first.
  - A regression test (or at minimum a documented manual test plan)
    under `clients/intellij/src/test/` drives: open project →
    edit descriptor → assert the restart hook was called within the
    debounce window.

**Notes**
- This sits **only** on the client side. The Rust server
  (`crates/lsp`) stays untouched — cycling the subprocess is the
  cleanest reload semantics and avoids in-server cache-invalidation
  races. Server-side hot-reload of `PureModel` is the long-term play
  but requires the incremental-compilation work in T-20260513-01 to
  land first, and even then the trigger should still come from the
  client (via `workspace/didChangeConfiguration` or equivalent).
- IntelliJ LSP API surface to lean on:
  - `com.intellij.platform.lsp.api.LspServerManager` — exposes a
    `stopAndRestartIfNeeded(LspServerDescriptor)`-style entry point
    for Ultimate-tier LSP integrations. Confirm exact method name
    against the API version pinned in `clients/intellij/build.gradle*`.
  - `com.intellij.openapi.vfs.AsyncFileListener` or
    `BulkFileListener` (project-scoped, subscribed via
    `MessageBusConnection`) — listen for VFS events on the descriptor
    path; cheaper than a raw filesystem watcher and integrates with
    the IDE's "external change" detection.
- Files to watch (priority order):
  1. The path stored in `LegendPureSettings.classpathToml` if
     non-blank (user-configured override).
  2. Otherwise: the ancestor-walk result starting from the project
     root for `legend-pure-classpath.toml`.
  3. Optionally: the snapshots directory the descriptor points at,
     for `.purem` rebuilds — but a `.purem` swap on its own is the
     harder case and probably better deferred until T-20260513-01
     enables chunk-scoped invalidation. First cut: descriptor-only.
- Embedded-fallback case: if the cascade resolved to the embedded
  platform (no on-disk descriptor), there's nothing to watch and
  this feature is a no-op. The plugin should detect that case at
  startup and skip registering the watcher.
- Settings UX: consider a "Restart server on classpath change"
  toggle in `LegendPureConfigurable` (default: on). Power users who
  are mid-edit on the descriptor and don't want the churn can opt
  out and fall back to manual restart.
- Adjacent: T-20260513-01 (incremental compilation) — once the
  server can invalidate per repo/chunk, the plugin's reload story
  can shift from "kill the subprocess" to "send a notification and
  let the server re-read the descriptor in place". Out of scope here;
  this TODO is the bridge until that lands.

<!-- agent-audit:start id=T-20260513-02 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
<!-- agent-audit:end -->

### T-20260513-01 — LSP recompiles the whole workspace on every change; needs incremental (chunk-scoped → element-scoped) compilation

- **Type:** perf
- **Area:** lsp | pure | runtime
- **Priority:** P1
- **Reporter:** Rafael
- **Filed:** 2026-05-13

**Summary**
Today the LSP server reruns the full compilation pipeline against the
entire workspace on every text-document change. As the platform model
grows (244 files / ~1660 elements compile-clean today per
`legend-pure-rust/CLAUDE.md`) latency on typing in the IDE compounds
linearly with workspace size. Two-stage improvement:
1. **Chunk-scoped recompile** — only re-hydrate the edited chunk and
   the transitively-dependent downstream chunks. Upstream chunks are
   reused as-is.
2. **Element-scoped recompile (long term)** — push dependency tracking
   down to the `ElementId` level so a body edit on one function only
   reruns the validators/lowerers that observed that element. This
   is the right shape for a Salsa-style demand-driven query engine;
   evaluate adopting Salsa (or a hand-rolled equivalent) as the
   compilation cache layer.

**Repro / Context**
- Edit any `.pure` file in an IntelliJ session with `legend lsp`
  attached. Observe the diagnostic/hover/completion latency grows
  with workspace size, not with the size of the local edit.
- Acceptance criteria:
  - **Phase 1 (chunk-scoped)**: on a single-file edit, only the
    edited chunk + chunks that import/depend on it are rerun.
    Measured by instrumenting the pipeline and asserting
    `len(rerun_chunks) ≤ 1 + len(downstream(edited_chunk))` in a
    targeted integration test.
  - **Phase 2 (element-scoped, Salsa or equivalent)**: a body-only
    edit on a function `f` whose signature is unchanged reruns only
    Pass-2b lowering for `f` and its dependents; Pass-1 shells,
    Pass-2a signatures, and validators on unrelated elements are
    cache-hit. Demonstrated by an LSP integration test that snapshots
    the recompiled-element set across a typing burst.
  - No regression in `cargo test --workspace`, `cargo lint-lib`,
    `cargo lint`, or in the existing LSP test suite. Surveyor
    (`eval_surveyor_root_strict_pass`) and PCT strict-pass tests
    remain green.

**Notes**
- Today's seam: `legend lsp` (`crates/cli`) → `crates/lsp` → calls into
  `crates/pure` end-to-end compile each tick. Identify the per-change
  entry point and route it through a cached `PureModel` that tracks
  per-chunk staleness.
- Pass-1 shell creation already populates every syntactic AST field
  (per the "Validators next to the data" memory note), which is the
  precondition for chunk-scoped invalidation — a downstream chunk
  whose imports' shells haven't changed can be considered up-to-date
  without rebuilding its dependencies.
- Salsa evaluation criteria (Phase 2): query granularity (per
  `ElementId`? per pass + `ElementId`?), interaction with the existing
  `Arc<PureModel>` snapshot model, cost of plumbing through the
  recursive-descent parser + pass-1/2a/2b pipeline, behaviour on
  hot reload during DAP sessions. Decide *before* implementing —
  a wholesale Salsa migration is large enough to be a BACKLOG item,
  not a TODO.
- Adjacent: the `cargo build` cold-path already feels this — DSL
  `.purem` rebuilds drive 5–15 min link times (`CLAUDE.md` build-time
  opt-ins section), and the `LEGEND_PURE_SKIP_DSL_SNAPSHOTS=1`
  escape hatch is a tell that the build graph isn't fine-grained
  enough. Phase 2 likely benefits both the LSP and the build script.
- Don't conflate parse caching with semantic caching. The parser is
  already fast; the wins are in Pass-2a (signature hydration), Pass-2b
  (body lowering), and the validator suite — those are the heavy
  passes that touch every chunk on a full recompile today.
- Test infrastructure: existing LSP integration tests under
  `crates/lsp/tests/` are the right home for chunk-scoped assertions.
  Phase 2 will likely need a new harness that records the set of
  recompiled query nodes per edit.

<!-- agent-audit:start id=T-20260513-01 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
<!-- agent-audit:end -->

### T-20260512-07 — Audit `eval_tests.rs` for coverage already gated by surveyor `<<test.Test>>` runs

- **Type:** refactor
- **Area:** runtime | tests
- **Priority:** P2
- **Reporter:** Rafael
- **Filed:** 2026-05-12

**Summary**
`crates/runtime/tests/eval_tests.rs` (~4053 lines, ~161 `#[test]`s) has
accumulated many Rust-seam tests that re-prove behaviour already covered
by platform `<<test.Test>>` functions under
`legend-pure-core/legend-pure-m3-core/src/main/resources/platform/`. The
stack-agnostic `eval_surveyor_root_strict_pass` test (and the PCT
strict-pass tests beside it) already gate `fail == 0 && error == 0`
across every platform `<<test.Test>>` and `<<PCT.test>>` in the model,
so duplicated Rust seams add cost (build + run time, maintenance) without
adding signal. Audit `eval_tests.rs` and remove tests whose behaviour is
fully specified by a platform test that the surveyor already runs, so
`cargo test --workspace` (and especially `-p legend-pure-runtime --test
eval_tests`) is as lean as possible.

**Repro / Context**
- File: `crates/runtime/tests/eval_tests.rs`
- The exemplar is already in the file at lines 4047–4053: the
  `toMultiplicity` (T-20260512-05) note removes the dedicated Rust seam
  on the explicit grounds that the 11 platform `<<test.Test>>` functions
  in `platform/pure/essential/lang/cast/toMultiplicity.pure` are already
  driven by `eval_surveyor_root_strict_pass`. Apply that same reasoning
  systematically to the rest of the file.
- Acceptance criteria:
  - A documented per-test audit (in the PR or a short scratch doc) of
    every `#[test]` in `eval_tests.rs`, classified as:
    (a) duplicated — fully gated by an existing surveyor / PCT strict
    test → delete; (b) partially duplicated — keep only the Rust-seam
    bit that isn't covered by a `<<test.Test>>` (e.g. asserting on an
    error *message* that Pure-side `->meta::pure::functions::asserts::*`
    can't introspect); (c) unique — keep, with a one-line comment
    pointing at what makes it Rust-specific.
  - Net reduction in `eval_tests.rs` line count + `#[test]` count, with
    `eval_surveyor_root_strict_pass`, `eval_pct_essential_strict_pass`,
    and `eval_pct_grammar_functions_strict_pass` still green.
  - No drop in surveyor pass count (current baseline per CLAUDE.md:
    surveyor 246/0/0, PCT 465/465). Any test moved out must have a
    corresponding `<<test.Test>>` proving its assertion — file new
    platform tests under the appropriate `platform/pure/.../*.pure`
    location if the Rust-side test was covering a gap.
  - Tests that genuinely need the Rust seam (e.g. inspecting heap
    shape, `Rc::ptr_eq` identity, `ObjectHandle` reachability,
    native-error string formatting that isn't observable from Pure)
    stay, but each survivor carries a one-line `// rust-seam: <reason>`
    comment so future audits can skip them quickly.

**Notes**
- Watch for false positives: a test may *look* duplicated but actually
  assert on something the surveyor can't see — error messages, panic
  vs. `PureException`, heap-internal invariants, native dispatch
  resolution path. Per memory `feedback_stack_specific_tests.md`,
  stack-specific assertions belong in this file; only redundant
  behaviour-only checks should leave.
- The relational/DSL populator wiring in
  `eval_surveyor_root_strict_pass` is what makes it possible to drop
  per-DSL Rust seams — verify each candidate-for-deletion is in fact
  reached by that run (a `<<test.Test>>` in an un-loaded repo would
  be silently skipped).
- Adjacent files to consider in the same sweep:
  `crates/runtime/tests/path_eval_smoke.rs`,
  `string_to_tds_smoke.rs`, `platform_invariants.rs` — same question
  applies, but smaller surface.
- Memory: see `project_surveyor.md` and
  `reference_surveyor_diagnostics.md` for the surveyor architecture
  and the histogram / missing-natives diagnostics already in
  `eval_tests.rs`.

<!-- agent-audit:start id=T-20260512-07 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
<!-- agent-audit:end -->

### T-20260512-06 — DAP debugger renders variables with Rust-internal `Debug` shape; needs Pure-syntax view + navigable structure

- **Type:** feature
- **Area:** dap | clients-intellij
- **Priority:** P2
- **Reporter:** Rafael
- **Filed:** 2026-05-12

**Summary**
When the debugger stops on a breakpoint, the IDE's Variables panel
shows raw Rust `Debug` formatting for every non-primitive `Value`
variant — `RefCell`, `RuntimeObject`, `InstanceId`, `PureDate`, etc.
leak straight through. The renderer should produce something close
to Pure surface syntax, and structured values (objects, collections)
should expand to individually-navigable children rather than
collapsing to a giant one-line string.

**Repro / Context**

```pure
let f = ^abc::Class2(prop = ^abc::Class1(propA = 12), prop3 = 123);
```

Today renders (in the Variables panel):

```
Object(RefCell { value: Dynamic(RuntimeObject { classifier: "abc::Class2",
properties: {"prop": [Object(RefCell { value: Dynamic(RuntimeObject {
classifier: "abc::Class1", properties: {"propA": [Integer(12)]},
bootstrap_element: None }) })], "prop3": [Integer(123)], "prop2":
[EnumValue { enum_id: InstanceId { chunk_id: 1, local_idx: 456 }, member: "A" }]},
bootstrap_element: None }) })
```

Wanted (sketch, refine at triage):

```
f : abc::Class2 = ^abc::Class2(...)
  ├─ prop  : abc::Class1[1] = ^abc::Class1(propA = 12)
  │    └─ propA : Integer[1] = 12
  ├─ prop2 : abc::Abc[1] = abc::Abc.A
  └─ prop3 : Integer[1] = 123
```

Second example:

```pure
let c = %2020;
```

Today: `Date(PureDate { inner: 2020-01-01T00:00:00, precision: Year })`

Wanted: `c : Date[1] = %2020`  (i.e. the literal that *produced* it,
or as close as the precision allows — `%2020-03`, `%2020-03-14`,
`%2020-03-14T09:30:00Z`).

Acceptance criteria:
- No `RefCell`, `Rc`, `Dynamic`, `RuntimeObject`, `InstanceId`,
  `PureDate`, `Arc`, `SmolStr` tokens appear in any DAP `Variable`
  payload sent to the client.
- Primitives serialise as their Pure literal form: `12`, `1.5`,
  `true`, `'hello'` (single-quoted), `%2020-03-14`, `%9:30:00`,
  `[1, 2, 3]`.
- Objects expose a top-line summary
  (`^Class2(...)` or `abc::Class2`) plus child `Variable`s — one
  per property — each with its own `variablesReference` for further
  drill-down. Children carry type + multiplicity in the type field
  so the IDE shows them in the Type column.
- Enum values render as `abc::Abc.A`, not as
  `EnumValue { enum_id: InstanceId { ... }, member: "A" }`.
- Collections (`[*]`) expose a length and indexable children
  (`[0]`, `[1]`, …) rather than a flat joined string.
- Cycles (objects reachable from themselves via a property chain)
  don't blow the stack — depth cap or visited-set.

**Notes**
- Two render seams today, both `Debug`-based fallthrough:
  - `crates/dap/src/server.rs:550` — `render_value`, primitives
    only; all non-primitives fall to `format!("{other:?}")`.
  - `crates/dap/src/hooks.rs:306` — duplicated copy noted in its
    doc comment as "Mirrors `server::render_value` byte-for-byte".
  De-dupe as part of this work — one renderer, two call sites.
- DAP protocol seam: `handle_variables`
  (`crates/dap/src/server.rs:317`) is currently a stub. This is
  where the structured-children path needs to be implemented:
  each parent `Value` mints a `variablesReference`, and follow-up
  `variables` requests resolve it to the property list. Existing
  precedent in rust-analyzer / metals: keep a per-frame map from
  reference id → `Value` snapshot, expire when the frame pops.
- Reuse the `compose` crate where possible for literal forms —
  string escaping, date precision, `^Class(...)` construction
  syntax already round-trip through it.
- Object rendering should read property *names* from the model
  (`PureModel` carries `Class.properties` with declared names),
  not from the runtime `RuntimeObject.properties` keys directly,
  to preserve declared order and surface unset properties as
  `[]` instead of hiding them.
- Cycle handling: visited set keyed by `Rc::as_ptr` on
  `ObjectHandle` (the heap identity per the
  "Heap design (Rc-managed)" memory note). Skip already-visited
  with `^Class2(... <cycle>)`.
- IntelliJ side: once the server sends structured children, the
  Platform Debugger UI handles tree expansion natively — no
  client-side code change expected. Verify in `clients/intellij`.
- Tests: snapshot tests for the rendering of each `Value` variant
  + a small integration test that drives a real
  `breakpoint → stack → variables → variables` sequence over the
  DAP protocol and asserts no Rust-internal tokens leak.
- Adjacent: hover / inlay-hint rendering in `crates/lsp/` likely
  has similar leaks — once the policy lands here, factor the
  renderer into a small `legend-pure-runtime::display` module
  (or similar) that both DAP and LSP can call.

<!-- agent-audit:start id=T-20260512-06 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
- 2026-05-13 — landed in commit `a71696071`. New `runtime::display`
  module materialises a `DisplayTree` of owned strings on the eval
  thread during pause; the DAP server thread serves
  `variables(reference)` lookups against that tree. `Value` stays
  `!Send` and never crosses the thread boundary.
- Architecture: `EvalHooks::before_eval` now returns `bool` (snapshot
  request) and a new `pause_with_snapshot(&mut self, src, tree)`
  delivers the rendered tree. The evaluator drives the renderer via
  its existing `EvalContext`; a `rendering_depth: u32` guard on the
  evaluator silences the hook during render-driven `call_function`
  callbacks so `toRepresentation` / `properties` calls never trigger
  a nested pause. Same heap, same context, same model — no second
  evaluator needed.
- Renderer: leaves go through
  `meta::pure::functions::string::toRepresentation` (single source of
  truth for `12`, `'hello'`, `%2020-03-14`, `abc::Abc.A`, etc.).
  Objects enumerate properties via a reflective call to
  `meta::pure::functions::meta::properties($obj->genericType())` so
  the property walk (declared + association + inherited) lives only
  in platform Pure source — no Rust duplicate. Collections expose
  indexed `[i]` children; maps expose entry children. Cycles caught
  via `Rc::as_ptr` visit set; depth cap 64, breadth cap 200.
- DAP wire: `Variable` gained `type`, `namedVariables`,
  `indexedVariables`; `PauseSnapshot.locals` is now
  `tree: DisplayTree`; `handle_scopes` reads `tree.root` per pause,
  `handle_variables` is a flat lookup on `tree.nodes`. The two
  duplicated `render_value` stubs at `server.rs:550` and
  `hooks.rs:306` are deleted.
- Verification: five `runtime::display` unit tests, five
  `runtime/tests/display_integration` tests (primitives, objects,
  inheritance, cycles, re-entry guard with no infinite recursion,
  banned-token sweep on every node). Full workspace `2085/2085`
  passes. `cargo fmt --check`, `cargo lint`, `cargo lint-lib`,
  `scripts/check-copyright.sh` all clean for the new code.
- Not run: IntelliJ manual smoke (Variables panel walkthrough on an
  actual breakpoint). Integration tests cover the tree the renderer
  emits, but the live DAP round-trip into the IDE is unverified
  end-to-end. CLAUDE.md UI policy: flag this if it matters.
  Status: Fix landed.
<!-- agent-audit:end -->

### T-20260512-03 — Virtual filesystem surfacing "decompiled" elements from `.purem` binaries

- **Type:** feature
- **Area:** lsp | clients-intellij | runtime | compose
- **Priority:** P2
- **Reporter:** Rafael
- **Filed:** 2026-05-12

**Summary**
Goto-definition / find-usages / hover currently work for elements that
live in user `.pure` files, but break the moment the target lives
inside an embedded `.purem` binary (platform M3, DSL metamodels,
generated tests slice). The user lands on "no source" because the
canonical-path → URI resolver only knows about on-disk files. Want a
read-only **virtual filesystem** that re-synthesises Pure source for
the in-memory model and serves it under a `pure:` URI scheme so the
editor can open, scroll, fold, and (importantly) goto-def *within*
the decompiled view — matching IntelliJ's "Decompiled class file"
experience for `.class` files.

**Repro / Context**
- Today: goto-def on `String` / `meta::pure::metamodel::type::Class`
  / a Mapping-DSL symbol returns a location with a canonical path
  like `/platform/pure/grammar/m3.pure` that the resolver can't map
  to a real file URI (it's a build-script-extracted snapshot path
  embedded in the binary). The editor opens nothing.
- Acceptance criteria:
  - LSP server advertises a `pure:` URI scheme (or
    `legend-pure-decompiled:` to be explicit) and answers
    `legend/virtualDocument` (or equivalent custom request) with the
    re-emitted source for any `(repo, canonical_path)` pair backed
    by an embedded `.purem`.
  - `textDocument/definition`, `references`, `hover`, `documentSymbol`
    on a virtual URI all work — i.e. the model is fully addressable
    from the synthetic source.
  - Re-emitted source is **structurally faithful** (same elements,
    same names, same supertypes, same property types) but does not
    need to be byte-identical to the upstream `.pure` (no comment
    preservation, no trivia round-trip).
  - IntelliJ side: register a `FileType` + read-only
    `VirtualFileSystem` for the `pure:` scheme so the platform-LSP
    integration's "open at location" path resolves cleanly.
- Out of scope (V1):
  - Editing virtual buffers — read-only is fine.
  - Preserving original whitespace / comments — the source `.purem`
    drops trivia, so there's nothing to preserve.
  - Cross-repo navigation into `.purem` blobs that aren't in the
    current classpath — defer to a follow-up.

**Notes**
- Compose seam: `legend_pure_compose` already round-trips AST →
  grammar text for every element kind we ship. Re-emission is
  "model → AST → compose" — the AST→compose step exists; the
  model→AST step is the missing piece, but `crates/protocol` does
  almost exactly this for JSON serialisation and can be a template.
- Canonical-path → repo lookup already exists in
  `Workspace::file_uri_for_canonical` (`crates/lsp/src/workspace.rs:218`);
  extend it to fall back to a virtual `pure:` URI when the canonical
  path doesn't map to a filesystem file but *does* exist in a loaded
  repo's element table.
- Server-side: a custom `legend/virtualDocument` request keyed by
  `(canonical_path)` returning `{ uri, languageId: "pure", text }`
  matches how rust-analyzer / metals serve decompiled / generated
  sources today.
- IntelliJ side: see `clients/intellij/src/main/kotlin/...` —
  needs a `VirtualFileSystem` impl + `FileType` registration; the
  Platform LSP API will route opens to it once the scheme is
  registered.
- Useful for **Find Usages** too — landing on a use-site in a
  `.purem`-embedded file becomes navigable instead of a dead end.
- Related: T-20260511-07 (Find Usages, just shipped) surfaces hits
  whose `canonical_path` lives in `.purem` and currently get dropped
  by `file_uri_for_canonical` (returning `None`). A virtual FS
  would un-drop them.
- Not a parity gap with Java — Java's IDE story for `.par` binaries
  is the upstream `legend-engine` plugin, not this repo.

<!-- agent-audit:start id=T-20260512-03 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
<!-- agent-audit:end -->

### T-20260512-02 — IntelliJ status-bar widget for LSP health / troubleshooting

- **Type:** feature
- **Area:** clients-intellij | lsp
- **Priority:** P2
- **Reporter:** Rafael
- **Filed:** 2026-05-12

**Summary**
When the LSP is silent (no diagnostics, no hover, no goto-def), the
user has no fast way to tell whether the server is alive, what state
it's in, or what went wrong. Today the answers live in the IntelliJ
"Logs" tab + `log_message` notifications, which is fine for a
support engineer but not for the first-line "is the plugin broken?"
question. Want a status-bar widget that surfaces LSP health at a
glance.

**Repro / Context**
- First-run failure modes already encountered:
  - No `legend` on `$PATH` → plugin starts, no LSP features, no
    obvious signal.
  - No filesystem repo in classpath → `initialized` logs a warning
    (`server.rs:217-229`) but the user only sees it if they
    happen to open the Logs view.
  - Compile produces N errors → `recompile_and_publish` logs
    `"recompile complete: N error(s)"` via `log_message` at
    `server.rs:113-118`, but again only visible in Logs.
- Widget surface area (sketch — refine at triage):
  - **State icon** (green / yellow / red): server running &
    compile clean / running but compile has errors / server not
    reachable.
  - **One-line summary**: `"Legend Pure: 0 errors, 9 repos, 244
    files"` or `"Legend Pure: 12 errors in 3 files"` or
    `"Legend Pure: server not started"`.
  - **Click-through**: opens a panel showing the resolved
    classpath path, the repo summary built at
    `server.rs:182-213`, last compile timestamp, last log lines,
    and a "Restart server" action.
- Acceptance criteria:
  - Widget renders on the IntelliJ status bar whenever a `.pure`
    file is open.
  - Updates on every `recompile_and_publish` cycle (the same
    seam that fires `log_message` today).
  - The "Restart server" action drops + re-spawns the
    `legend lsp` subprocess.

**Notes**
- Implementation seam: `clients/intellij/src/main/kotlin/...` —
  add a `StatusBarWidgetFactory` + `StatusBarWidget` impl, wire
  it through `plugin.xml`. Subscribes to LSP notifications
  (`window/logMessage`, `window/showMessage`,
  `textDocument/publishDiagnostics`) — the server already emits
  these.
- Server-side: consider adding a small `legend/lspStatus`
  custom notification carrying the per-compile summary
  (error count, repo count, files count) so the widget doesn't
  have to scrape free-form `log_message` strings. Symmetric with
  the existing `legend.run` / `legend.runTest` /
  `legend.runPCT` / `legend.listPctAdapters` workspace commands
  (`server.rs:147-161`).
- Adjacent: the "No filesystem repos in classpath" warning at
  `server.rs:217-229` is *the* canonical first-run failure
  signal. The widget should foreground it (red badge + tooltip)
  rather than rely on the modal `showMessage` popup that's easy
  to dismiss.
- Not a parity gap with Java — Java has no LSP / IDE story in
  this repo.

<!-- agent-audit:start id=T-20260512-02 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
<!-- agent-audit:end -->

### T-20260512-01 — Wire H2 environment into GitHub CI so Rust tests pass

- **Type:** devops
- **Area:** ci | store-relational-runtime
- **Priority:** P1
- **Reporter:** Rafael
- **Filed:** 2026-05-12

**Summary**
The new H2 relational backend (commits `539163e5c27`, `3c0310a3409`,
`567d4461e05`) reads runtime config from
`crates/store-relational-runtime/src/config.rs` — a jar path
(`LEGEND_PURE_H2_JAR`), version, PG-protocol port, and a `java`
binary — sourced from env vars or a `[extension.relational.h2]`
classpath block. The local dev defaults are checked into
`.cargo/config.toml:65-68`, but the GitHub Actions runner has none
of these, so `legend-pure-store-relational-runtime`'s integration
tests either skip silently or fail when CI runs the workspace gates.

**Repro / Context**
- Local: `cargo nextest run -p legend-pure-store-relational-runtime`
  is green; CI green-or-red status for the same package is unknown
  (and may be silently passing because the tests skip when the jar
  isn't on disk).
- Expected: CI runs the H2 integration tests against a real H2 jar
  + a `java` runtime that can launch the embedded PG server.
- Need to verify whether GitHub Actions' default Ubuntu image
  ships a JDK on `$PATH` (recent `actions/runner-images` includes
  Temurin 17 / 21, but version may need pinning) and whether the
  H2 jar needs to be downloaded per-job or cached.

**Notes**
- CI workflow lives at `.github/workflows/rust.yml`. Today the
  pipeline is format-check → lint-lib → lint → llvm-cov nextest →
  doctests → bench. The H2 setup step has to land **before**
  nextest, and ideally guard so the rest of the workspace still
  runs when H2 isn't reachable (matrix or feature flag).
- Two shapes worth weighing at triage:
  1. **Download H2 jar + use system JDK.** `setup-java@v4` for
     JDK pinning + a `curl` to fetch the jar, cached via
     `actions/cache@v4` keyed on the H2 version. Cheap, no
     container.
  2. **Service container.** Run H2 as a Docker service alongside
     the job. Heavier-weight; only worth it if we need a long-
     running shared instance across jobs.
  Approach 1 is the natural first cut — matches the local-dev
  shape (jar-on-disk + spawned `java` subprocess) and reuses
  the existing env-var contract.
- Env vars to set in the CI step:
  - `LEGEND_PURE_H2_JAR=/path/to/cached/h2-2.1.214.jar`
  - `LEGEND_PURE_H2_VERSION=2.1.214`
  - `LEGEND_PURE_H2_PG_PORT=` (pick a free port, or 0 for OS-assigned)
  - `LEGEND_PURE_H2_JAVA=java`
- Risk: port collision on shared runners. The current default
  `1975` was picked locally; CI should either let the engine pick
  a free port (preferred) or use an env-substituted random port
  per job.
- Sweep: when this lands, also audit whether the embedded-jar
  classpath cascade (`extension.relational.h2` in
  `legend-pure-classpath.toml`) needs a sibling CI sample so
  workspace integration tests that read it (not just env vars)
  don't go untested.

<!-- agent-audit:start id=T-20260512-01 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
<!-- agent-audit:end -->

---

## Closed

<!-- Resolved / migrated / wontfix items, newest first. Keep the full block
     including the final audit-block status for posterity. -->

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
- 2026-05-13 — claimed by claude-opus-4-7[1m] — plan approved as
  `plan-to-solve-todo-jaunty-church.md`. Scope narrowed: the
  cross-chunk validators (`validate_repo_visibility`,
  `validate_access_levels`, …) already produce per-file errors with
  use-site `source_info`, so no compiler-side work is needed. The
  actual gap is publish-side: `recompile_and_publish` ignored
  `Workspace::file_uri_for_canonical` and had no stale-clearing for
  non-open URIs. Incremental re-validation + CLI sibling +
  virtual-scheme for synthetic sources deferred as separate
  follow-ups (see plan §"Out of scope").
- 2026-05-13 — diagnosis:
  * The old publish path at `server.rs:88-104` had a `format!("file://{canonical}")`
    fallback that produced malformed URIs (`file://myproj/foo.pure`
    treats `myproj` as host, not path) — so the code that *looked* like
    cross-file publishing didn't actually work.
  * There was no `previously_published` tracker, so any non-open file
    that had errors cleared by an edit would keep its stale red
    squiggles on the client forever.
  * `Workspace::file_uri_for_canonical` (`workspace.rs:218`) already
    knew how to resolve every Filesystem-repo canonical to a real
    on-disk URI, and returned `None` for synthetic/embedded sources —
    so we already had a correct synthetic-source filter; we just
    weren't calling it.
- 2026-05-13 — implementation:
  * `crates/lsp/src/handlers.rs`: new pure function
    `plan_diagnostics_publish(workspace, previously_published) ->
    PublishPlan { entries, next_previously_published }`. Computes the
    per-cycle publish set as (resolved diagnostics ∪ open buffers ∪
    stale URIs from prev cycle), with resolved diagnostics overriding
    empty placeholders. Synthetic sources are filtered by
    `file_uri_for_canonical` returning `None`. 6 unit tests in
    `mod publish_plan` cover empty workspace, open-buffer-no-errors,
    unopened-file-resolution, synthetic-source filter,
    stale-clearing, and open-buffer-with-errors-publishes-once. One
    end-to-end integration test
    (`cross_file_error_surfaces_on_unopened_file`) drives a real
    `Workspace::compile()` against a 2-file descriptor-rooted user
    project alongside the embedded platform: breaks A via open-buffer
    overlay, asserts B's URI surfaces with a non-empty diagnostic
    even though B is not open, then fixes A and asserts the
    stale-clear empty publish for B. Pins the structural assumption
    that compile-emitted `source_info.source` is attributed to the
    *use-site* file (B), not the *defining* file (A).
  * `crates/lsp/src/server.rs`: `Backend` gains
    `previously_published: Arc<Mutex<HashSet<Uri>>>`.
    `recompile_and_publish` rewritten to (a) hold workspace + prev
    locks together for compile + plan, (b) update prev tracker
    in-line, (c) drop both locks before the `publish_diagnostics`
    awaits. `did_close`'s explicit clear left as-is (next recompile
    re-establishes correct state via the global publish path) — see
    plan §"smaller concern" note for a follow-up to revisit
    close-semantics under workspace-wide intent.
  * `cargo test -p legend-pure-lsp`: 37/37 pass.
    `cargo lint-lib` + `cargo lint` zero new warnings. Manual
    IntelliJ end-to-end verification per plan §Verification
    deferred to the user.
- 2026-05-13 — IntelliJ end-to-end revealed the server-side fix
  was necessary but not sufficient: JetBrains' native LSP
  integration (`com.intellij.platform.lsp`) renders
  `publishDiagnostics` only into per-editor `MarkupModel`s via
  `LspDiagnosticsSupport`, so notifications for unopened files
  were cached but never displayed. Three independent gates had
  to be cleared on the plugin side:
  * **`Lsp4jClient` wrapper** — new
    `clients/intellij/.../WorkspaceAwareLspNotificationsHandler.kt`
    decorates the platform's `LspServerNotificationsHandler`
    (installed via `PureLspServerDescriptor.createLsp4jClient`
    override, Kotlin `by` delegation), forwarding every
    `publishDiagnostics` to the platform delegate **and** to the
    two surfaces below.
  * **`ProblemsCollector` direct call** — new
    `LegendPureFileProblem.kt` implements
    `com.intellij.analysis.problemsView.FileProblem`;
    `ProblemsCollector.getInstance(project)
    .problemAppeared(p)` / `.problemDisappeared(p)` is called
    directly. Critically, `ProblemsListener.TOPIC` was tried
    first and had no effect — the topic has no platform-
    registered subscribers for this purpose. The stock
    `ProblemsViewHighlightingWatcher` also calls its stored
    listener instance directly, not via the topic. Identity-
    tracked per URI in a `ConcurrentHashMap` so each
    `problemDisappeared` carries the same instance the earlier
    `problemAppeared` published.
  * **`WolfTheProblemSolver` filter gate** — new
    `PureProblemFileHighlightFilter.kt` registered as
    `<problemFileHighlightFilter>` in `plugin.xml`. Without
    this, `WolfTheProblemSolverImpl.reportProblems` silently
    returns at its internal `isToBeHighlighted(file)` gate and
    the file never enters Wolf's problem set
    (`isProblemFile=false` after a successful-looking call —
    the diagnostic signal that pointed at this missing EP).
- 2026-05-13 — docs + memory:
  * `clients/intellij/README.md` — new *Workspace-wide
    diagnostics* section + Problems tool window step in the
    smoke test + updated Layout listing.
  * `crates/lsp/src/lib.rs` — module-level doc for
    `publishDiagnostics` now notes workspace-wide publish +
    plugin wrapper.
  * Auto-memory: `reference_intellij_lsp_diagnostics.md`
    captures the three-gates pattern; `reference_lsp_publish_scope.md`
    captures the server-side publish-plan shape. Future
    sessions debugging "diagnostics not showing" should find
    the answer in minutes.
- 2026-05-13 — verified working end-to-end in the IntelliJ
  sandbox against `legend-pure2`: a single user edit that
  breaks 469 cross-file references surfaces every affected
  file in the Problems tool window's *Project Errors* tab and
  paints red filename badges in Project view.
- 2026-05-13 — moved to Closed.
  Status: Feature shipped.
<!-- agent-audit:end -->

### T-20260512-04 — Property default values not applied at `^Class(...)` instantiation (multiplicity violation)

- **Type:** parity-gap
- **Area:** runtime | compiler
- **Priority:** P1
- **Reporter:** Rafael
- **Filed:** 2026-05-12

**Summary**
A property with a default value should be auto-populated when the
caller omits it in a `^Class(...)` constructor. Today the multiplicity
validator fires instead, treating the property as unset.

**Repro / Context**

```pure
Class abc::Class2 {
    prop: abc::Class1[1];
    prop2: abc::Abc[1] = abc::Abc.A;
    prop3: Integer[1];
}

^abc::Class2(prop = ^abc::Class1(propA = 12), prop3 = 123);
```

Actual error:

```
Error instantiating class 'Class2'.  The following properties have
multiplicity violations: 'prop2' requires 1 value, got 0
```

Expected: `prop2` is populated from its declared default
(`abc::Abc.A`); no multiplicity violation; instance constructed
successfully.

Java-parity reference: upstream Java Pure applies property defaults
during `new`/`^Class(...)` construction. Pattern matches the same
defaulting that already works for top-level `function` parameter
defaults (existing tests cover that path).

**Notes**
- Likely seams:
  - Constructor lowering — wherever `ExprKind::NewInstance` lowers
    to runtime instantiation, the per-property default expression
    should be inserted for any unsupplied property *before* the
    multiplicity check fires.
  - Or the property-defaults pass — if defaults are stored on
    `Property` but ignored at runtime construction, the runtime
    side is what needs the fix.
- Validation order is load-bearing: defaults must be substituted
  *before* multiplicity validation, otherwise the validator sees
  zero values and (correctly) rejects.
- Related TODOs (same family — `^Class(...)` semantics):
  - T-20260511-02 — required properties not checked
    (zero-value violation in the *intended* direction).
  - T-20260511-03 — supplied value not type/multiplicity-checked.
  - T-20260511-04 — unknown property names not rejected.
  This one is the inverse of T-20260511-02: same code path, opposite
  symptom (defaults should pre-empt the "missing" verdict).
- Test seed: add to `crates/runtime/tests/` (or the closest
  existing `^Class(...)` integration test) — fixture with three
  classes (`Class1`, `Abc` enum-or-class, `Class2`), omit the
  default-bearing property, assert it materialises to the default
  value at runtime.

<!-- agent-audit:start id=T-20260512-04 -->
<!-- Agents: append entries below. Do not rewrite the developer block above. -->
- 2026-05-12 — root cause: `^Class(...)` and Pure-source
  `new(class, id, [keyExpr])` share `finish_construction`
  (`crates/runtime/src/native/lang.rs:662`), which applied caller-
  supplied triples but never walked the class hierarchy for
  unspecified properties' `default_value` expressions. The third
  constructor overload, `DynamicNew` (same file), already did this
  correctly at lines 1236–1279 — the two paths had drifted, and the
  compile-time check `ConstructorMissingRequiredProperty` correctly
  short-circuits on defaulted properties so the divergence only showed
  at runtime.
- Fix: extracted `apply_property_defaults(ctx, obj, class_id,
  supplied_keys)` as a shared private helper in
  `crates/runtime/src/native/lang.rs`. `finish_construction` now
  back-fills unsupplied properties from their `default_value` between
  `apply_property_triples` and the deferred multiplicity validation.
  `DynamicNew::execute` was refactored to call the same helper —
  structural fix that eliminates the drift point per the
  `feedback_no_tactical_hacks` memory.
- Canonical tests: added five `<<test.Test>>` functions to
  `legend-pure-core/.../platform/pure/grammar/functions/lang/creation/new.pure`
  under `meta::pure::functions::lang::tests::newAtRuntime`:
  primitive default, primitive default overridden, enum default,
  enum default overridden, inherited default. They run on every
  stack's surveyor (Java + Rust). Rust surveyor: 385/0/0 (was 380),
  including the five new ones. Full `cargo test -p
  legend-pure-runtime`: green. `cargo test -p legend-pure-parser-pure`:
  green. `cargo fmt --check`, copyright check: clean.
- Pre-existing limitation preserved (mirrors `DynamicNew`):
  `populate_association_inverses` runs over caller triples only, not
  over defaults. Out of scope for this TODO; file a follow-up only if
  default-valued association ends become a real use case.
- 2026-05-13 — moved to Closed. Fix landed on `legend-pure-rust` as
  commit `26e738c7ebb` ("fix(runtime): apply property defaults at
  `^Class(...)` (T-20260512-04)").
  Status: Fix landed.
<!-- agent-audit:end -->

### T-20260512-05 — `toMultiplicity` doesn't compile + missing test coverage

- **Type:** parity-gap
- **Area:** compiler | runtime
- **Priority:** P1
- **Reporter:** Rafael
- **Filed:** 2026-05-12

**Summary**
`meta::pure::functions::lang::toMultiplicity<T|z>(source:T[*],
object:Any[z]):T[z]` (declared in
`platform/pure/essential/lang/cast/toMultiplicity.pure:17`) doesn't
compile end-to-end today, and the Rust workspace has no direct test
of it — existing references in `crates/pure/tests/integration_tests.rs`
(2783, 2811) only exercise it as a *vehicle* for testing
`UndeclaredMultiplicityParameter` diagnostics, not the function's
own behaviour. Need both: get it compiling and add positive +
negative test coverage.

**Repro / Context**
- Native signature (PCT):
  ```pure
  native function <<PCT.function>>
  meta::pure::functions::lang::toMultiplicity<T|z>(
      source:T[*], object:Any[z]
  ):T[z];
  ```
- Java-parity reference: `toMultiplicity` is the multiplicity
  analogue of `cast` — narrows a `T[*]` to whatever multiplicity
  the second argument carries (`z`). At runtime it validates the
  source's actual cardinality against `z` and either returns the
  narrowed collection or throws.
- Acceptance criteria:
  - User code like:
    ```pure
    function test::demo(xs: String[*]): String[1] {
      $xs->toMultiplicity(@String[1])
    }
    ```
    compiles cleanly (no dispatch error, no
    `UndeclaredMultiplicityParameter`, no return-type mismatch).
  - Runtime native body validates cardinality:
    - `xs` has exactly `z` values → return them at multiplicity `z`.
    - Cardinality mismatch → throw a Pure exception with the same
      shape as Java Pure's (`"Multiplicity Many doesn't match
      [1]"` etc — copy the wording from upstream).
  - Test coverage in both seams:
    - **Compiler** (`crates/pure/tests/`): positive cases for `[1]`,
      `[0..1]`, `[2..*]`; negative cases for arity / type mismatch
      and undeclared mult parameter.
    - **Runtime** (`crates/runtime/tests/` or `eval_tests.rs`):
      cardinality-validates-and-returns, cardinality-fails-with-
      exception, identity on `T[*] → T[*]`.

**Notes**
- Declaration lives in upstream Java platform sources at
  `legend-pure-core/legend-pure-m3-core/src/main/resources/platform/
  pure/essential/lang/cast/toMultiplicity.pure:17` — shared between
  Java and Rust stacks.
- The compile-failure root cause is unconfirmed — possibilities:
  - Dispatch failing to resolve the parametric multiplicity `z`
    from `object:Any[z]` argument position (consumer of `z` is
    the *return* type, which means `z` has to flow from the
    second arg's mult into the result, not from the first arg).
  - No native registered in `crates/runtime/src/natives/` — but
    that would only show up at runtime, not compile time.
  - Pass 2b body lowering failing on `@T[z]` second-arg shape
    (multiplicity-annotated cast literal).
  Diagnose before writing the fix.
- Adjacent natives (same multiplicity-coercion family): `toOne`,
  `toOneMany`, `cast` (type coercion sibling). Use their dispatch
  + native registration pattern as the template.
- Two integration tests already lean on `toMultiplicity`
  (`undeclared_mult_inside_function_type_errors`,
  `declared_mult_function_signature_clean`) — they should keep
  passing after the fix; if they currently rely on the function
  *not* compiling, rewrite them around a different placeholder.
- See MEMORY note "Split natives by call shape" — one native per
  declared signature, no `pos2_is_*` probing inside a single
  native body.

<!-- agent-audit:start id=T-20260512-05 -->
- 2026-05-12 — claimed by claude-opus-4-7[1m] — plan approved as
  `splendid-bubbling-fairy.md`; option B (value-at-mult calling
  form) chosen over parser work for `@[m]` bare-mult literal.
- 2026-05-12 — diagnosis: three independent issues, only one of
  which is actually in scope here.
  * The TODO repro `xs->toMultiplicity(@String[1])` is **not
    valid Pure** — the Java grammar (`M3CoreParser.g4:311`)
    permits `AT (type | multiplicity)` only, never the combined
    `@T[m]`. The Rust parser currently *accepts* `@T[m]` but
    splits it into 2 args (`@T` + `[m]` as a collection literal),
    which is what made the existing 2783/2811 integration tests
    "work" — they only check the `UndeclaredMultiplicityParameter`
    diagnostic, not dispatch resolution.
  * Java parity calls toMultiplicity as `xs->toMultiplicity(@[m])`
    (bare multiplicity literal — `AbstractTestToMultiplicity.java`).
    The Rust parser does not yet accept `@[m]`. Filed as a
    follow-up (separate scope) — not blocking behavioural parity.
  * The native body was never registered, so even the working
    "value-at-mult" form (`$xs->toMultiplicity($hint)` where the
    hint's static type carries the desired multiplicity) failed
    runtime dispatch with `Function not found:
    toMultiplicity_T_MANY__Any_z__T_z_`.
- 2026-05-12 — implementation:
  * `crates/runtime/src/native/collection.rs`: new `ToMultiplicity`
    struct + `NativeFunction` impl. Reads `args[1].type_info.multiplicity`
    (statically inferred `z`), counts the source collection size,
    validates against the bounds, and either returns the source
    unchanged or raises `EvaluationError` with the verbatim
    Java-parity message `"Cannot cast a collection of size N to
    multiplicity [BOUNDS]"`. Helper functions `multiplicity_bounds`
    + `format_multiplicity_bounds` handle every `Multiplicity`
    variant. Registered as
    `toMultiplicity_T_MANY__Any_z__T_z_` — the dispatcher emits
    the generic name (no per-`z` specialisation), so a single
    registration covers all call sites.
  * `platform/pure/essential/lang/cast/toMultiplicity.pure`:
    upgraded native stereotypes from `<<PCT.function>>` to
    `<<PCT.function, PCT.platformOnly>>` matching upstream parity
    (matches `assertError`, `replaceAll`, `keyValues`, …). Added
    8 `<<test.Test>>` functions: 5 positive (`testToMultiplicityToOne`,
    `…ToZeroOne`, `…ToOneMany`, `…IdentityZeroMany`, `…ToExactN`)
    + 3 negative (`…FailsFromEmptyToOne`, `…FailsFromManyToOne`,
    `…FailsFromEmptyToOneMany`). Negative tests use `assertError`
    with the exact Java-parity error text.
  * Tests use the "value-at-mult" calling form: pass any value
    whose static multiplicity supplies `z`. e.g. `'hint'` → `[1]`,
    `[1,2,3]->first()` → `[0..1]`, `[1,2]->toOneMany()` → `[1..*]`,
    `[1,2]->concatenate([3])` → `[*]`. Documented in the file's
    header comment along with the pointer to the bare-mult-literal
    parser follow-up.
- 2026-05-12 — Rust integration tests in
  `crates/runtime/tests/eval_tests.rs`: 7 new tests pinning the
  native at the runtime seam (4 positive multiplicities + 3
  negative cardinality-mismatch error texts) + 1 surveyor proof
  (`surveyor_to_multiplicity_tests_all_pass`) that runs the
  `meta::pure::test::surveyor::runTestsFromPath` entry against
  `meta::pure::functions::lang::tests::toMultiplicity` and asserts
  `error=0, fail=0, pass>=8`. The two existing diagnostic tests at
  `integration_tests.rs:2783` / `:2801` still pass — they remain
  vehicles for `UndeclaredMultiplicityParameter` and never relied
  on `toMultiplicity` actually dispatching.
- 2026-05-12 — Java tests retired (3 files removed):
  * `legend-pure-core/.../AbstractTestToMultiplicity.java`
  * `legend-pure-runtime/legend-pure-runtime-java-engine-compiled/.../TestToMultiplicity.java`
  * `legend-pure-runtime/legend-pure-runtime-java-engine-interpreted/.../TestToMultiplicity.java`
  All 5 `@Test` methods (`testErrorFromToOne`, `testErrorFromToOneMany`,
  `testMatchExact`, `testBigger`, `testError`) are covered by the
  new Pure `<<test.Test>>` functions and the Rust integration
  tests. No other Java code referenced these classes (`grep -r
  AbstractTestToMultiplicity` clean).
- 2026-05-12 — verification:
  * `cargo test -p legend-pure-runtime --test eval_tests
    to_multiplicity`: 7/7 PASS.
  * `cargo test -p legend-pure-runtime --test eval_tests
    surveyor_to_multiplicity_tests_all_pass`: PASS (8/8 surveyor).
  * `cargo test -p legend-pure-parser-pure --test integration_tests
    -- undeclared_mult_inside_function_type_errors
    declared_mult_function_signature_clean`: 2/2 PASS (regression
    guard for the integration tests at 2783/2801).
- 2026-05-12 — follow-ups (audit notes, not separate TODOs filed):
  * ~~**Parser support for bare-multiplicity literal `@[m]`.**~~
    ✅ **Landed 2026-05-12 (same day).** Sibling track to `@T`
    added across the full pipeline: AST variant
    `MultiplicityReferenceExpr` (`crates/ast/src/expression.rs`),
    parser peek-after-`@` (`crates/parser/src/parser/expression.rs:614`),
    semantic `ExprKind::MultiplicityReference`
    (`crates/pure/src/types.rs`), lowering with eager
    `type_info = Any[m]`
    (`crates/pure/src/lower/type_ref.rs::lower_multiplicity_reference`),
    runtime eval arm materialising the
    `meta::pure::metamodel::multiplicity::Multiplicity` heap wrapper
    (`crates/runtime/src/eval.rs`; reuses
    `native/meta.rs::build_multiplicity_wrapper` after refactor to
    take `&mut RuntimeHeap`), composer
    (`crates/compose/src/expression.rs`), and a protocol-v1 stub
    emitting `Var { name: "@[<mult>]" }` with a
    `TODO(protocol-multiplicity-literal)` marker. Platform tests in
    `toMultiplicity.pure` now carry 11 `<<test.Test>>` functions:
    4 positive `@[m]` + 3 negative `@[m]` + 1 parametric `@[o]` +
    3 value-at-mult equivalence (all forms are canonical Pure
    semantics — `Any[z]` accepts any value of any type whose static
    multiplicity is `m`, so platform-level coverage runs on every
    stack's surveyor, not just Rust). No per-test Rust integration
    seams or scoped surveyor proof were added: the existing
    `eval_surveyor_root_strict_pass` test already enforces
    `fail == 0 && error == 0` across every `<<test.Test>>` in the
    model, which includes the 11 new ones. Parser snapshot test
    `multiplicity_literal_concrete_and_named` locks the AST shape
    (unique signal — covers a layer the surveyor doesn't).
  * **`legend compile` vs runtime auto-imports divergence.** The
    CLI's `crates/cli/src/commands/compile.rs:61` and the runtime
    test fixture's `crates/core-platform-pure/src/platform.rs:38`
    use *different* auto-import lists — `legend compile`
    misses `meta::pure::functions::lang` etc., which made the
    canonical TODO repro fail at the CLI but succeed inside the
    runtime tests. Surfaced during diagnosis; out of scope for
    this TODO.
  * **Protocol JSON shape for `@[m]`.** Current stub emits
    `Var { name: "@[<mult>]" }` — structurally valid but lossy.
    Upstream Java protocol-v1 doesn't define a dedicated value-spec
    shape (the Java parser elaborates to a `Multiplicity`
    instance); future work: settle on a faithful shape and update
    `crates/protocol/src/v1/convert.rs` accordingly. Tagged with
    `TODO(protocol-multiplicity-literal)` at the call site.
  Status: Fix landed.
<!-- agent-audit:end -->

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
- 2026-05-12 — claimed by claude-opus-4-7[1m] — element-level V1
  scope agreed with reporter; implementation on worktree
  `feat/T-20260511-07-find-usages`.
- 2026-05-12 — discovery: reverse-reference index already existed
  (`crates/pure/src/refs.rs::ReferenceIndex::usages_of`,
  `crates/pure/src/refs.rs:170`), built per-compile in
  `Workspace::references` at `crates/lsp/src/workspace.rs:173`.
  Only the LSP plumbing was missing — capability advertisement,
  request dispatch, cursor → ElementId resolver. No
  `crates/pure/` changes needed.
- 2026-05-12 — implementation:
  * `crates/lsp/src/server.rs`: add `references_provider:
    Some(OneOf::Left(true))` capability + dispatch
    `textDocument/references` to `handlers::references_for_position`.
    Returns `Ok(None)` for empty results (LSP 3.17 shape; matches
    "no references found" UX in IntelliJ).
  * `crates/lsp/src/handlers.rs`: new `references_for_position`.
    Two-tier cursor resolution — reference-site click via
    `ReferenceIndex::find_at`, declaration-site click via
    `model.locate` + inclusive-end span check (new helper
    `cursor_in_source_info_inclusive`). The existing
    `cursor_in_source_info` is LSP-style exclusive-end, which
    rejects clicks on single-char names like `Class abc::A`
    (`name_source_info` is the degenerate `c13-c13`); the new
    helper matches `refs::contains`'s inclusive-end semantics.
  * `clients/intellij/README.md`: Find Usages added to supported
    features. No Kotlin change — IntelliJ's LSP runtime
    auto-binds `textDocument/references` to ⌥F7 once the
    capability is advertised.
- 2026-05-12 — V1 scope (`target_element: None` cases): cursor on
  `PropertyCall` / `QualifiedPropertyCall` / `Variable` returns
  empty `Vec`. Pinned by `references_empty_on_property_call_v1_limitation`
  test so a future richer-target index doesn't silently flip
  behavior.
- 2026-05-12 — tests: 7 new in `handlers.rs::tests`:
  use-site click, declaration-site click, include_declaration
  toggle, whitespace cursor, property V1 limitation, cross-file
  URI resolution via the resolver, missing-index degraded path.
- 2026-05-12 — verification:
  * `cargo build --workspace`, `cargo nextest run --workspace`:
    2074/2074 tests pass.
  * `cargo lint-lib`, `cargo lint`, `cargo fmt --check`,
    `./scripts/check-copyright.sh`: clean (pre-existing dead-code
    warnings on `resolve_value_spec_target` in handlers.rs:213
    and minor pedantic warnings in `runtime/src/native/string.rs`
    tests are unrelated).
  * Platform purem round-trip
    (`legend-pure-snapshot-builder::smoke build_is_byte_deterministic`)
    green.
  Status: Fix landed (commit f08d3ad24bb).
- 2026-05-12 — follow-ups (audit notes, not separate TODOs cut yet):
  * **Property / qualified-property granularity.**
    `Reference.target_element` is `None` for `PropertyCall`,
    `QualifiedPropertyCall`, `Variable` in `crates/pure/src/refs.rs`
    (lines 490, 507, 522). Exposing property-level Find Usages
    requires either a richer target type (e.g. `ReferenceTarget`
    enum) or a sibling reverse-index keyed by `(ElementId,
    SmolStr)`. Triage candidate.
  * **`cursor_in_source_info` exclusive-end legacy.** The
    existing helper at `crates/lsp/src/handlers.rs:233` uses
    LSP-style exclusive-end semantics against compiler-emitted
    inclusive-end `SourceInfo` spans — a latent bug for clicks
    on the last character of any name. `definition_for_position`
    uses it; consider migrating to the new inclusive variant or
    aligning the two semantics workspace-wide.
  * **Rename refactoring (`textDocument/rename`).** Natural next
    feature on top of the same index; needs workspace-edit
    protocol shape. Out of scope here.
<!-- agent-audit:end -->

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

