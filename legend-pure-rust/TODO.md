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
