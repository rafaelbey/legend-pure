# A Pure Evaluator in Rust — MVP Proposal

> **Status: proposal, October 2026 (revised 2026-10-08).** This document is
> self-contained. It assumes the reader knows Legend Pure and legend-engine as
> they exist in Java today and nothing else. It describes what a minimum viable
> Rust evaluator is, which existing technologies it plugs into, and a sequence
> of steps where each step ships something usable and proves something
> specific.

---

## 0. Orientation for a fresh session

Read this first; everything below assumes it.

**Rules for this effort (set by the author, 2026-10-08).**

1. **Only the evaluator and the language metamodel it needs move.** Nothing
   else from earlier Rust work is brought along: no parser, no compiler, no
   LSP, no DSL crates. They stay on an archive branch.
2. **No other crates.** The new workspace is two crates — `metamodel` and
   `evaluator` — plus whatever a later step explicitly adds (a PELT reader, a
   JNI boundary). Each addition is named in its step.
3. **The evaluator depends on nothing Pure-shaped except the bootstrap.** No
   binary graph format, no `.pure` repository, no platform load. The
   bootstrap is the M3 metamodel itself (primitives, `Any`, `Nil`, and the
   classes of `m3.pure`), which the evaluator needs to answer `type()`,
   `instanceOf` and dispatch questions.
4. **Natives are fished one at a time, when a test or a PCT failure asks for
   one.** No native file moves wholesale.
5. **Tests build the evaluator's inputs directly** — literals, collections,
   function expressions, lambdas, classes, functions — and hand them to the
   evaluator. No source text, no loader. The PELT import comes *after* the
   evaluator is proven this way **and after the Java boundary and the PCT
   adapter exist**, so every PELT increment is measured from its first commit.
6. **Much of this already exists.** Appendix A says, file by file, what to
   take from the archive and what to strip. Extraction is a copy with listed
   edits, not a rewrite.

| What | Where |
|---|---|
| This document | `legend-pure-rust/docs/rust-evaluator-mvp.md` on branch `legend-pure-rust` of the `legend-pure` fork (`rafaelbey/legend-pure`). It is the single source of truth for the Rust direction. |
| The Rust workspace **after Step 0** | `legend-pure-rust/` (Cargo workspace, edition 2024) holding `crates/metamodel` and `crates/evaluator`. Build and test: `cargo build --workspace`, `cargo test --workspace`. `legend-pure-rust/CLAUDE.md` is rewritten in Step 0 to match. |
| The Rust workspace **before Step 0** (the archive) | Tag `archive/pre-evaluator-extract-2026-10` and branch `legend-pure-rust-archive`, both at the last commit before extraction. Every path in Appendix A is an archive path. |
| The Java stack (upstream FINOS code) | Sibling directories at the repository root: `legend-pure-core/`, `legend-pure-runtime/`, `legend-pure-maven/`, … Requires JDK 11 or 17 and Maven. |
| The bootstrap source | `legend-pure-core/legend-pure-m3-core/src/main/resources/platform/pure/grammar/m3.pure`, embedded at compile time by the metamodel crate. This is the one `.pure` file the evaluator may depend on. |
| PELT files for the platform, locally (on the Java classpath from Step 2, for PCT test discovery; read by Rust from Step 3) | `legend-pure-core/legend-pure-m3-core/target/classes/**/*.pelt` plus `org/finos/legend/pure/module/platform.{pmf,psr,pxr,pbr,pfn}`. Produced by `mvn -T 4 install -DskipTests -pl legend-pure-core/legend-pure-m3-core -am` (15–30 min first time). ~1,900 files, 3.5 MB. |
| PELT byte-level reference | `docs/architecture/pelt-serialization.md` at the repository root — on branch `docs/pelt-format`, submitted upstream as finos/legend-pure PR #1360. **Not on `legend-pure-rust` yet**: `git cherry-pick c989b50953b` (that single commit) before Step 3. Do **not** merge the branch — it tracks upstream `master` and differs from `legend-pure-rust` by ~700 files. |
| Background analysis (why PELT, what it contains, how it maps) | `legend-pure-rust/docs/pelt-analysis.md` (moved from `docs/deferred/` in Step 0). Sections 6.3–6.8 are the source of Steps 2–7 here. |
| legend-engine | Checkout at `~/Projects/legend-engine` (currently on branch `rust-parser`; `upstream/master` is the FINOS main line — see Appendix C for which branch is the target; it matters from Step 2, when the adapter is registered there). Its `*-pure` modules emit PELT into their jars (e.g. 414 `.pelt` in `functions-standard`); its PCT adapters are the `Test_*_PCT.java` classes (93 on `upstream/master`); the engine compiler is `legend-engine-language-pure-compiler` (`toPureGraph`); the Pure function libraries are `legend-engine-core/legend-engine-core-pure/legend-engine-pure-code-functions-{standard,relation,json,planExecution,…}`. |
| PCT in the platform | Profile `meta::pure::test::pct::PCT` in `legend-pure-core/legend-pure-m3-core/src/main/resources/platform/pure/essential/tests/pct_core.pure`; ~470 `<<PCT.test>>` functions in the platform sources; Java report providers `Essential_Functions_PCTReportProvider`, `Grammar_Functions_PCTReportProvider`, `Core_{Compiled,Interpreted}_PCTReportProvider`; shared helpers in `m3/pct/shared/PCTTools.java`. |
| What to extract, and what to leave | Appendix A (extraction map). Read it before Step 0 and before fishing anything later. |
| Step 0 mechanics | Appendix B: archive commands, workspace skeleton, gates. |
| Decisions still owned by the author | Appendix C. Do not guess these; ask. |

**Definition of done for the MVP.** The "Rust interpreted" PCT adapter
(registered in Step 2, red) reports pass rates for the platform and at least
one `legend-engine-pure-code-functions-*` module from PELT-loaded code with a
published ranked list of missing natives (Step 5's gate), and a static Pure
function runs against an engine-compiled model with Java-equal results
(Step 6's gate). Step 7 is the first increment after the MVP.

---

## 1. Thesis

Legend already has a complete Pure **compiler** in Java, and legend-engine
already has its own compiler that turns user models (protocol JSON) into a Pure
graph. What a Rust effort can add first, with the least risk, is an
**evaluator**: a component that executes Pure functions.

Three facts make an evaluator-first approach possible:

1. **An evaluator can be proven on its own.** Its inputs are a handful of
   node shapes — literals, function expressions, variables, lambdas — plus
   class and function declarations. Tests can build those shapes directly and
   check the result, with no source text and no loader. The only thing the
   evaluator must know up front is the M3 metamodel, which is small and fixed.
2. **The Java build already writes the compiled graph to disk in an open,
   element-granular binary format called PELT.** Every `*-pure` module in
   legend-pure and legend-engine emits it. It contains function bodies, not just
   signatures, so once the evaluator exists, real programs can be executed from
   it without any `.pure` source and without re-compiling. PELT, and every Java
   Pure object, share one shape: a classifier plus a bag of named property
   values. An evaluator that understands that shape can consume graphs built by
   the Java compiler, by legend-engine's compiler, or by anything else.
3. **PCT (Platform Compatibility Testing) is an existing, executable
   definition of "behaves like Java".** Pure functions tagged `<<PCT.function>>`
   and their `<<PCT.test>>` tests run on both Java engines on every build. A
   third engine that runs the same tests gets a per-function parity score for
   free.

So the MVP is: **stand the evaluator up over the bootstrap alone; connect it
to Java and register it with PCT while it still fails every test; then read
PELT and watch the pass rate climb.** No Rust parser or compiler is needed for any step below.
Natives (functions declared `native` in Pure and implemented in Java) must be
re-implemented in Rust; the target is 100 %, and PCT turns that into a ranked,
incremental backlog rather than a precondition.

---

## 2. The landscape this plugs into

```mermaid
flowchart LR
    subgraph Build time (Java, exists today)
        SRC[.pure sources\nplatform + engine repos] --> JC[Java Pure compiler]
        JC --> PELT[(PELT files in jars)]
        JC --> BYTECODE[generated Java\ncompiled engine]
    end
    subgraph Request time (Java, exists today)
        PJ[protocol JSON\nuser models + queries] --> EC[legend-engine compiler\ntoPureGraph]
        EC --> UG[user graph:\nJava CoreInstance objects]
        PELT --> MP[MetadataPelt\nlazy platform graph]
        MP --> UG
    end
    subgraph Proposed (Rust)
        M3[m3.pure\nbootstrap] --> RE[Rust evaluator]
        TB[hand-built nodes\ntests] --> RE
        PELT --> RE
        UG -. JNI node bridge .-> RE
        RE -. JNI values / proxies .-> JAVA[Java callers]
        PCT[PCT suites] --> RE
    end
```

| Technology | What it is | How the evaluator uses it |
|---|---|---|
| **M3 bootstrap** | `m3.pure`: the metamodel's own classes (`Class`, `Property`, `Function`, `ValueSpecification`, …) in an instance-graph syntax, plus the primitive types. | The only Pure input the evaluator has before Step 3. Gives `type()`, `instanceOf`, property typing and dispatch something to stand on. |
| **PELT** | Per-element binary files (`<path>.pelt`) plus per-module metadata (`.pmf` manifest, `.psr` sources, `.pxr` external refs, `.pbr` back references, `.pfn` function names), produced by the `compile-pure` Maven goal into `target/classes` and therefore into every jar. Byte-level reference: `docs/architecture/pelt-serialization.md` in the legend-pure repository. | The source of all *static* code: platform functions, engine core functions, DSL metamodels. Read-only input, from Step 3. |
| **Java Pure compiler** | `legend-pure` M3 compiler. | Stays the compiler for all repository code. Not replaced. |
| **legend-engine compiler** | `toPureGraph`: protocol JSON → Java `CoreInstance` graph (handcrafted `Root_..._Impl` objects). | Stays the compiler for user models, mappings, queries. Its output is consumed live through JNI. |
| **Java engines** | compiled (generated bytecode) and interpreted (tree-walker over `CoreInstance`s). | Remain the reference implementations; PCT compares against them. |
| **PCT** | `<<PCT.function>>` / `<<PCT.test>>` functions, run per engine through *adapters*. | A new "Rust interpreted" adapter gives parity and native-coverage numbers. |
| **JNI** | The Java ↔ native boundary. | Entry point (`evaluate(function, args)`), value marshalling, and the bridge that lets Rust read Java objects. |

---

## 3. Vocabulary

- **Element.** A packageable Pure element: class, enumeration, profile,
  association, function, measure, package. One PELT file per element.
- **Node.** Any instance in the graph: an element, a property, a type reference,
  an expression inside a function body. A node is `(classifier path, name?,
  source span?, property → values)`. Values are primitives, references to other
  nodes in the same element (by index), or references to nodes in other elements
  (by **reference id**).
- **Bootstrap.** The model the evaluator starts with: the primitive types,
  `Any`, `Nil`, and the M3 classes parsed from `m3.pure`. Fixed, small, built
  once per process.
- **Executable form (IR).** The shapes the evaluator actually consumes: an
  *expression tree* (`ValueSpec` carrying an `ExprKind`) and *element tables*
  (`Element::Class`, `Element::Function`, …). Section 5, Step 1 gives the
  M3 ↔ IR correspondence.
- **Building block.** One IR node built directly in Rust — by a test, or
  later by a lowering — rather than parsed from text.
- **Reference id.** A string identity for a node across elements: the element's
  path for an element, otherwise the element path plus a navigation path, e.g.
  `meta::pure::metamodel::type::Class.properties['name']` or
  `my::f__String_1_.expressionSequence[0].parametersValues[1]`. Stable across
  builds.
- **Static vs dynamic.** *Static* is everything compiled at build time and
  shipped as PELT. *Dynamic* is what legend-engine builds per request from user
  input: classes, mappings, queries, lambdas, as live Java objects.
- **Lowering.** Turning the generic node shape into the executable form. For
  declarations this is building type and property tables; for function bodies
  this is building an expression tree.
- **Native.** A Pure function declared without a body. Each needs a Rust
  implementation.
- **Fishing.** Copying one existing implementation (typically one native, or
  one module) from the archive branch into the workspace because a test or a
  PCT failure needs it now. The unit of fishing is the smallest thing that
  compiles.

---

## 4. The MVP in one picture

```mermaid
flowchart TD
    subgraph Inputs
        BOOT[Bootstrap\nm3.pure + primitives]
        TB[Hand-built nodes\ntests, Step 1]
        STORE[PELT store\ndirectory · jar list · classloader]
        JOBJ[Java objects\nengine-built graph]
    end
    subgraph metamodel crate
        TAB[Element tables\nclasses · functions · packages]
        IR[Expression tree\nValueSpec / ExprKind]
    end
    subgraph evaluator crate
        EVAL[Evaluator\nvalues · heap · variables · lambdas · dispatch]
        NAT[Native registry\nfished one at a time]
    end
    subgraph Added in Step 2
        JNI[JNI layer\nevaluate() · values · handles · typed failures]
        PCTA[PCT adapter\n"Rust interpreted" · red first]
    end
    subgraph Added in Steps 3–5
        DIR[Element directory + lowering\nPELT nodes → tables + trees]
    end
    BOOT --> TAB
    TB --> TAB
    TB --> IR
    STORE --> DIR --> TAB
    JOBJ --> DIR
    DIR --> IR
    TAB --> EVAL
    IR --> EVAL
    NAT --> EVAL
    EVAL <--> JNI
    PCTA --> JNI
```

Design rules that hold for every step:

- **Two crates.** `metamodel` (element tables, expression tree, bootstrap) and
  `evaluator` (values, heap, variables, dispatch, native registry). Later steps
  add a crate only when the step says so.
- **Bootstrap only.** The evaluator's model starts as chunk 0 — the bootstrap —
  and nothing else. Every further element arrives either from a test's
  builders (Step 1) or from a lowering (Steps 4 onward).
- **Natives on demand.** The registry starts empty. A native is fished when a
  test or a PCT failure names it; the registry's contents are always
  explainable by a test.
- **Red before green.** The PCT adapter is registered (Step 2) before any
  loader exists, with failures typed by kind. Its pass rate is the progress
  metric; a step that does not move it needs a reason.
- **Lazy by element.** The directory (manifests) is read eagerly; an element's
  nodes are read and lowered the first time something touches it. Platform plus
  engine is tens of thousands of elements; a request touches hundreds.
- **One node interface.** Everything downstream of "give me this node's
  classifier and properties" is identical whether the node came from PELT bytes
  or from a live Java object. This is what lets the engine's compiler output be
  consumed without a Rust compiler.
- **Identity by path.** Elements are identified by their Pure path everywhere.
  A Java object that is a packageable element with a path the evaluator already
  knows *is* that element, not a copy of it.
- **Java stays the oracle.** Any divergence is a bug on the Rust side until
  proven otherwise; PCT is the arbiter.

---

## 5. Steps

Each step has a goal, what gets built, how it integrates, what it proves, and
the gate that says it is done. Steps are ordered so that every one ships a
usable artifact.

### Step 0 — Extract the evaluator and its metamodel into a clean workspace

**Goal.** A workspace that contains exactly the evaluator, the metamodel it
needs, and this document. Anyone opening it finds one direction and no code
arguing for another. Everything else is recoverable from the archive.

**Assumption (confirm with the author; Appendix C item 5).** The live branch's
`legend-pure-rust/` directory is *replaced* by the new two-crate workspace.
The previous workspace — parser, compiler, LSP, DSL crates, binary snapshot
format, CLI — lives on the archive tag and branch only. Same path, so the
repository-level `CLAUDE.md` and tooling pointers stay valid.

**Build.**

1. *Freeze.* Tag the current head and create the archive branch (commands in
   Appendix B). Nothing is lost.
2. *Empty the directory.* Remove everything under `legend-pure-rust/` except
   `docs/rust-evaluator-mvp.md` and `docs/deferred/pelt_vs_purem.md` (renamed
   to `docs/pelt-analysis.md`).
3. *Skeleton.* Root `Cargo.toml` with two members, `crates/metamodel` and
   `crates/evaluator` (Appendix B has the skeleton). Dependencies allowed:
   `smol_str`, `thiserror`, `rust_decimal`, `im-rc`, `jiff`, `tracing`. No
   `serde`, no `postcard`, no `linkme`, no `toml`.
4. *Copy the metamodel crate* per Appendix A, table 1, applying the listed
   edits: strip `Serialize`/`Deserialize` derives, copy the two leaf types from
   the old AST crate instead of depending on it, drop the compiler-only fields
   and methods from the model type, extract the 31-line least-upper-bound
   function instead of the 5.9 k-line module it sits in. Keep the `include_str!`
   of `m3.pure`; that *is* the permitted bootstrap, and the relative path is
   unchanged because the crate sits at the same depth as before.
5. *Copy the evaluator crate* per Appendix A, table 2: values, heap, variable
   context, dates, errors, hooks, the native registry (without its plugin
   slice), the evaluator itself with the listed arms stubbed as "unsupported",
   and the 14 in-crate unit tests that already run over a bootstrap-only model.
   Natives: none. The two seed tests that call `plus` and `times` fail until
   Step 1 fishes them; that is expected and is the first thing Step 1 does.
6. *Docs.* New `legend-pure-rust/CLAUDE.md` (build, test, the six rules, a
   pointer here), `README.md` (one paragraph), nothing else. No backlog, no
   architecture doc; this document is the plan. **Also rewrite the
   repository-level `CLAUDE.md`** (its "Repository Shape" and "Current State"
   sections describe the archived workspace: parser, compiler, 17 CLI
   subcommands, `BACKLOG.md`, `docs/deferred/`); every session loads it first,
   so it is the one file that must not contradict this plan.
7. *Build plumbing outside the workspace.* `.github/workflows/rust.yml` runs
   `cargo fmt --check`, the `cargo lint-lib` / `cargo lint` aliases from the
   archived `.cargo/config.toml`, and `cargo llvm-cov nextest --workspace
   --exclude legend-pure-parser-stress --exclude legend-pure-parser-jni
   --fail-under-lines 85`. Replace with `cargo fmt --check`, `cargo clippy
   --workspace --all-targets -- -D warnings`, `cargo test --workspace`; drop
   the excludes (those crates no longer exist) and the coverage gate until
   Step 1 has a test base worth measuring. The Maven reactor lists two Java
   modules for the old JNI crate (`legend-pure-runtime/pom.xml`:
   `legend-pure-runtime-rust-evaluator-bindings-ap`,
   `legend-pure-runtime-rust-evaluator`); the latter's tests load
   `legend-pure-rust/target/debug/libpure_rust_jni*`, which Step 0 deletes.
   Remove both `<module>` lines in Step 0 (the directories stay; Step 2
   re-adds them with the fished JNI crate) so `mvn install` on the fork stays
   green.
8. *Agent memory.* Project memory entries that describe the old workspace's
   decisions are deleted or rewritten so future sessions start from this
   document. (Outside the repository; not part of the gate.)

**Gate.** `cargo build --workspace` succeeds with only the six dependencies
above in `cargo tree`; `grep -rE 'purem|pelt|legend_pure_parser|serde|linkme'
crates/` returns nothing (the `m3.pure` include path matches none of those
patterns); `cargo test --workspace` compiles (the two seed tests that call
`plus` and `times` fail until Step 1 fishes them); both `CLAUDE.md` files
(repository root and `legend-pure-rust/`) name `docs/rust-evaluator-mvp.md` as
the single source of truth and describe nothing from the archive as current;
`rust.yml` runs green on the new workspace; `mvn -T 4 install -DskipTests`
still succeeds at the repository root.

**Start here.** Appendix B, then Appendix A table 1 top to bottom, then table
2. Rough size: metamodel ≈ 6 k lines copied, evaluator ≈ 8 k lines copied;
one to two days including the edits.

### Step 1 — The evaluator over the bootstrap alone

**Goal.** Prove the evaluator executes every expression shape and consults
every element shape it knows, with inputs built in Rust and nothing loaded.

**M3 ↔ executable form.** Tests speak the evaluator's language; this table is
the dictionary from the M3 names a Pure reader knows to the shapes the tests
build. It is also the specification the Step 5 PELT body lowering must
satisfy.

| M3 node (what Java and PELT hold) | Executable form (what tests build) |
|---|---|
| `InstanceValue` with one primitive value | `ExprKind::{Integer,Float,Decimal,String,Boolean,Date}Literal` |
| `InstanceValue` with several values | `ExprKind::Collection { elements }` |
| `InstanceValue` whose value is an enum value | `ExprKind::EnumValue` |
| `InstanceValue` whose value is a packageable element / type | `ExprKind::PackageableElementRef`, `ExprKind::TypeReference`, `ExprKind::MultiplicityReference` |
| `SimpleFunctionExpression` (resolved `func`, `parametersValues`) | `ExprKind::FunctionCall(FunctionCallData { function: Some(id) \| None, function_name, arguments })` |
| `SimpleFunctionExpression` whose `func` is a `Property` | `ExprKind::PropertyCall` (receiver is `arguments[0]`) |
| `SimpleFunctionExpression` whose `func` is a `QualifiedProperty` | `ExprKind::QualifiedPropertyCall` (receiver is `arguments[0]`, then the arguments) |
| `VariableExpression` | `ExprKind::Variable { name }` |
| `LambdaFunction` (`InstanceValue` wrapping it) | `ExprKind::Lambda { parameters, body, … }` |
| `let` / `if` / `new` / `copy` desugared by the compiler | `FunctionCall` to the natives `letFunction_…`, `if_…`, `new_…`, `copy_…` |
| `Class` element (properties, generalizations, qualified properties) | `Element::Class` in a chunk ≥ 1, registered in its package |
| `ConcreteFunctionDefinition` (`expressionSequence`) | `Element::Function { body, is_native: false }` |
| `NativeFunction` | `Element::Function { is_native: true }` plus a registry entry under the mangled name |
| `Enumeration`, `Profile`, `Association`, `PrimitiveType`, `Package` | the matching `Element::*` variant |
| `genericType` / `multiplicity` on any `ValueSpecification` | `ValueSpec.type_info: Option<ResolvedType>` (may be `None`; the evaluator does not require it) |

**Build.**
- A public `builders` module in the evaluator crate, promoted from the test
  helpers that already exist (`lit_*`, `make_expr`, `test_model`): expression
  builders for every row above (`lit_int`, `collection`, `var`, `call`,
  `call_resolved`, `prop`, `qp`, `lambda`, `enum_value`, `type_ref`, …) and a
  `ModelBuilder` that starts from the bootstrap and allocates `Element::Class`
  / `Element::Function` / `Element::Enumeration` into chunk 1, registers them
  in their package, and rebuilds the derived indexes (specializations,
  association properties) before the evaluator is created.
- **One hidden contract the builders must honour.** Dispatch reads the
  *mangled* function name (`plus_Integer_MANY__Integer_1_`) from the element
  node's `name` field; the old compiler wrote it there. `ModelBuilder::function`
  therefore computes the mangled name from the signature (the mangling helper
  is part of the metamodel crate) and stores it as the node name. A function
  built with a plain name never dispatches. The Step 5 lowering hits the same
  wall; write it down once, here.
- Natives, fished in this order because the first tests need them: `letFunction`,
  `if`, `new` (four registry keys: a bare `new` plus three mangled shapes),
  `copy`; then `plus`/`minus`/`times`
  for the arithmetic tests; then `eq`/`equal`, `and`/`or`/`not`. Each fished
  native arrives with its own unit test. Nothing else.
- Error paths: variable not found, function not found (the mangled key is in
  the message), property not found, multiplicity violation.

**Integrates with.** Nothing external. This step is pure Rust.

**Proves.** The evaluator is a usable library on its own. Gate: one test per
`ExprKind` arm that is not stubbed (the stubbed arms — relation literals,
column specs, path literals — have one test each asserting the "unsupported"
error); one test per `Element` kind the evaluator reads; one test per dispatch
path (native by exact name, user function body, prefix fallback, qualified
property, lambda application, property access); `grep -rE 'purem|pelt|legend_pure_parser' crates/`
still returns nothing.

**Start here.** Move the 14 unit tests out of `eval.rs` into
`crates/evaluator/tests/building_blocks.rs`, make them pass by fishing `plus`,
then add rows from the table one at a time. Rough size: builders ≈ 500 lines;
each fished native is a copy plus one test.

### Step 2 — Values across JNI, and a PCT adapter that starts red

**Goal.** Connect the evaluator to Java and register it as a PCT engine
*before it can run anything real*. Every test is red on day one; from here
on, each later step is measured by how many turn green.

**Build.**
- JNI entry point: a Java class with `evaluate(functionPath, args...)` backed
  by a native method; a context object holding the model (bootstrap only, at
  this step) and the evaluator.
- Marshalling: Java primitives, strings, dates and decimals to and from Rust
  values; collections both ways.
- Handles for Rust-side instances: Java holds an opaque identifier, Rust keeps
  the object alive until Java releases it. Typed Java proxies are optional and
  come later.
- **Typed failures across the boundary.** A Rust failure reaches Java as an
  exception carrying a *kind* — `FunctionNotFound`, `Unsupported`,
  `AssertionFailed`, `RuntimeError` — plus the mangled name or source span.
  Without this, a red suite is noise: "not loaded yet" and "wrong answer"
  must be distinguishable in the report.
- **The "Rust interpreted" PCT adapter.** A Java test-suite builder that
  enumerates `<<PCT.test>>` functions from the Java-loaded graph (the existing
  PCT tooling already does this for the two Java engines), calls
  `evaluate(testPath)` for each, and maps the outcome: no exception → pass;
  `AssertionFailed` → fail; `FunctionNotFound` / `Unsupported` → fail, tagged
  with the kind. An expected-failures manifest (JSON, same shape as the Java
  engines' `pct_*.json`) lists known failures with their kind, so the CI job
  is green when every failure is an expected one. Registered twice: in the
  fork's Java module for the platform suite, and in legend-engine as a
  `Test_*_PCT` class for one functions module.
- A report: per module, pass and fail-by-kind counts, and the ranked list of
  `FunctionNotFound` names. That list is the native backlog from Step 5 on.

**Integrates with.** Any JVM with Pure jars on the classpath; the PCT harness
as it exists in legend-pure and legend-engine; the CI of both repositories if
the author chooses (Appendix C).

**Proves.** The boundary works and the measurement exists. Gate: the platform
suite and one engine functions module run end to end through the adapter with
every test reported as `FunctionNotFound` (nothing is loaded yet); no crash
and no handle leak under a loop of ten thousand calls; the expected-failures
manifest makes the job green; the report prints a 0 % line. From this commit
on, the number on that line is the progress metric of the whole effort.

**Start here.** The JNI crate and the Java module exist on the archive
(Appendix A, table 3): fish `crates/jni` as the third crate, cut its
dependencies on the archived parser, protocol, repository and codegen crates,
and re-add the two Maven `<module>` lines removed in Step 0. Precondition:
the Java side needs the platform PELT jar for test discovery, so the Maven
build from §0 (`mvn -T 4 install -DskipTests -pl legend-pure-core/legend-pure-m3-core -am`)
moves here; the engine module's jar comes from the legend-engine checkout.
Model the adapter on any `Test_*_PCT.java` in
`legend-engine-core/legend-engine-core-pure/legend-engine-pure-code-functions-standard`
(registration shape) and on
`legend-pure-runtime/legend-pure-runtime-java-engine-interpreted/src/main/java/org/finos/legend/pure/runtime/java/interpreted/Core_Interpreted_PCTReportProvider.java`
(report shape). Rough size: under 1 k lines Rust, ~600 lines Java.

### Step 3 — Read PELT

**Goal.** Decode every PELT file type into in-memory structures, with no
knowledge of Pure semantics.

**Build.**
- A reader for the element file: 16-byte envelope (signature, serializer
  version, reference-id version), raw-DEFLATE body, string table, then nodes
  in breadth-first order with their property bags.
- Readers for the module files — manifest, sources, external references,
  functions by name, the back-reference index, and the **per-element**
  back-reference files (one per referenced element; 616 for the platform) —
  each with its own signature and version.
- The reference-id parser (`path`, `.prop`, `.prop[i]`, `.prop['key']`,
  `.prop[key='v']`).
- A store abstraction over *where bytes come from*: a directory
  (`target/classes`), an ordered list of jars (a classpath), or a Java
  classloader reached over JNI. File naming lives only here.
- A small binary (`pelt-inspect`) that dumps one element and lists a module's
  elements from its manifest.

**Integrates with.** The jars Maven already builds. Nothing on the Java side
changes.

**Proves.** The format is fully understood. Gate: golden tests decoding every
file in the platform's `target/classes` without error and round-tripping the
string table; `inspect` output reviewed against the Java `print` of the same
element.

**Start here.** New leaf crate `crates/pelt` — the fourth crate, and the only
one this step adds (dependencies: `flate2`, a zip reader only; it does not
depend on `metamodel` or `evaluator`). The classloader-backed store uses the
Java module from Step 2: a `ClassLoader.getResource` callback exposed to
native code (~100 lines Java). Fixtures: copy a dozen representative
files from the platform `target/classes` (a class, an enumeration, a profile,
an association, a native function, a test function with a lambda, `Root.pelt`,
`Integer.pelt`, the five `platform.*` module files) into
`crates/pelt/tests/fixtures/`. The Java `print` of an element comes from a
throwaway JUnit test in `legend-pure-m3-core` using `PureCompilerLoader` on
`target/classes`. Rough size: ~2 k lines.

### Step 4 — A static graph in memory

**Goal.** Load modules by name, resolve dependencies, and answer "what is `X`"
for every element without executing anything.

**Build.**
- Element directory from manifests: path → classifier, source span, module;
  follow manifest dependencies transitively (as `MetadataPelt` does).
- Declarative lowering: classes (properties, supertypes, type parameters),
  enumerations, profiles, associations, primitive types, function *signatures*.
  Produces the metamodel crate's element tables, which the evaluator already
  uses for `instanceOf`, property typing and dispatch. **Identity rule:** an
  element the bootstrap already defines (`String`, `Any`, `Class`, …) is the
  bootstrap's element; the PELT copy is skipped, not duplicated.
- Overload index from `.pfn` (simple name → functions); subtype and
  association-inverse indexes from `.pbr`, so no element has to be loaded to
  answer "who extends `X`".
- Packages: real ones from their PELT files, virtual ones implied by element
  paths.
- A generic "property-bag element" for any classifier the lowering does not
  know (DSL metamodels, engine extensions), so *every* element is loadable.

**Integrates with.** Module dependencies and repository boundaries exactly as
Java sees them; a store configuration pointing at a directory, jar list or
Maven coordinate.

**Proves.** Type-level questions match Java. Gate: for every element in the
platform manifest, classifier, supertypes, property names and types and
function signatures agree with what the Java `CoreInstance` API reports for the
same element.

**Start here.** Build the oracle first: a JUnit test in `legend-pure-m3-core`
that loads `target/classes` through `PureCompilerLoader` and dumps, per
element, `(path, classifier, supertypes, properties[(name, type, multiplicity)],
function signature)` as JSON into `crates/pelt/tests/fixtures/platform-oracle.json`.
The Rust gate is then a diff against that file. The lowering lives in the
evaluator crate (module `load/`), targets the metamodel crate's types, and
invents nothing new except the property-bag element: repurpose
`Element::DSLInstance`, replacing its opaque byte payload with a classifier
plus a property map. Rough size: 2–3 k lines.

**Keeping fixtures honest.** Step 3 fixtures and the oracle both derive from
`target/classes`, which only exists after a Maven build. Add
`legend-pure-rust/scripts/regen-pelt-fixtures.sh` that runs the Maven module
build, runs the oracle test, and copies the chosen files into
`crates/pelt/tests/fixtures/`; commit the fixtures and the script, and record
the legend-pure commit they were generated from in a `FIXTURES.md` next to
them. The oracle JUnit is committed on the fork branch under
`legend-pure-core/legend-pure-m3-core/src/test/java/.../pelt/` (a test, not
product code); whether it goes upstream is an open question (Appendix C).

### Step 5 — Execute function bodies, measure with PCT

**Goal.** Run Pure functions whose bodies come from PELT.

**Build.**
- Body lowering: M3 expression nodes (`SimpleFunctionExpression`,
  `InstanceValue`, `VariableExpression`, `LambdaFunction`, `KeyExpression`,
  type references) into the expression tree, row by row from the Step 1 table.
  Calls are already resolved to their target function by the Java compiler;
  types and multiplicities are already inferred on every expression. The
  lowering reads them, it does not recompute them. Function nodes get the
  mangled name (Step 1's hidden contract).
- Lazy per element: a function body is lowered on first call.
- Measurement is the Step 2 adapter: its report already splits failures by
  kind, so *missing native* (`FunctionNotFound` on a native declaration) and
  *lowering gap* (`Unsupported`) are visible per module without new tooling.
  Optionally, fish the archive's Rust-side PCT runner (Appendix A, table 3)
  for local iteration without a JVM; the published numbers are the adapter's.
- Natives: fished per histogram row, highest-leverage first. The `assert*` and
  `executePCTTest` support natives come first because every PCT test goes
  through them.

**Integrates with.** The PCT corpus as it exists in the platform `.pure`
sources, compiled by Java and read from PELT. No test is rewritten.

**Proves.** Semantic parity, function by function. Gate: the Step 2 adapter's
pass rate on the platform suite and on one engine functions module, with the
ranked list of missing natives; the expected-failures manifest shrinks with
every commit and the rate only goes up.

**How PCT works, in one paragraph.** A PCT function is a platform function
carrying `<<PCT.function>>`; its tests are functions carrying `<<PCT.test>>`
(~470 in the platform today) that call the function and assert on the result.
An *adapter* is a Pure function that decides how a test's expression is
executed on a given engine; each engine registers its adapter and a report
provider. A test passes when evaluating it raises no assertion. Per-engine
exclusions (known gaps) are declared in a manifest so the pass rate is honest
about what is skipped.

**Start here.** The body lowering is the new code; natives are fished.
First target: the first PCT test in the platform's essential functions that
needs only `assert`, `eq` and a literal. Rough size of the body lowering:
3–5 k lines.

**How natives become incremental.** The histogram is the backlog. Each native
is weighted by how many PCT functions it unblocks, so the order of work is
data-driven, and "100 % natives" is reached one measured step at a time
rather than demanded up front.

### Step 6 — Consume legend-engine's graph live

**Goal.** Execute static Pure code against user models compiled by
legend-engine, with no Rust compiler.

**Build.**
- A JNI-backed node source: a small Java helper that, given any `CoreInstance`
  (handcrafted `Root_..._Impl`, PELT-lazy, or interpreted), returns its
  classifier, keys and values in one call, primitives inline and other objects
  as handles. On the Rust side the same node interface as PELT, so the same
  lowering applies.
- **Identity unification.** When a crossing object is a packageable element
  whose path the evaluator already knows (`String`, `Any`, a platform class, a
  profile), substitute the known element. Without this, `instanceOf`, type
  equality and dispatch break. This is the one thing required before the first
  call.
- **Lazy definitions overlay.** When a value's class is unknown (a user class
  such as `my::Person`), fetch the class node, run the Step 4 declarative
  lowering, and register it in an overlay layer on top of the static graph.
  Instances never need lowering; only definitions the evaluator reasons about by
  type do.
- Lowering of engine-built lambdas (`LambdaFunction_Impl` trees) through the
  same body lowering, cached per Java object.
- Read-only in the first increment: no writes back into Java objects.

**Integrates with.** `PureModel` in legend-engine: pass its objects straight
into `evaluate`. Whole-model queries (`Class.all()`) use a bulk call into the
engine's indexes rather than walking.

**Proves.** The engine's compiler output is directly executable by Rust.
Gate: a worked example such as a class-analytics function from
`legend-engine-xts-analytics/legend-engine-xts-analytics-class/legend-engine-xt-analytics-class-pure`
run on an engine-compiled model with
results equal to the Java compiled engine's; the Step 2 adapter extended to
engine modules whose tests need a compiled user model.

**Start here.** The Java node helper lives next to
`PureRustEvaluator`; it needs only the `CoreInstance` interface
(`legend-pure-m4`), so it works for handcrafted, PELT-lazy and interpreted
objects alike. First target: a function whose inputs are classes and
properties only, before anything that touches mappings. Rough size: ~2 k
lines Rust (node bridge, identity unification, overlay), ~300 lines Java
(node helper).

### Step 7 — DSL instances and property bags end to end

**Goal.** Mappings, runtimes, connections, services and other DSL instances
from the engine flow through the evaluator into the static Pure code that
consumes them (routing, analytics, in-memory execution).

**Build.**
- Generic hydration of property-bag elements into evaluator instances, with
  the schema read from the DSL's metamodel class (itself static, from PELT):
  property names, types and multiplicities validate the bag. The archive's
  DSL-instance hydration module is the starting point (Appendix A, table 3).
- Typed fast paths only where profiling says so.

**Integrates with.** Every DSL legend-engine already compiles, without the
evaluator knowing any DSL grammar.

**Proves.** The evaluator is DSL-agnostic. Gate: a mapping-analytics or
in-memory relation function run on an engine-compiled mapping, parity against
Java. Rough size: ~1 k lines.

### Later, outside the MVP

- **Writing PELT from Rust.** Needed only when something other than the Java
  compiler produces graphs. Requires producing complete M3 node shapes and the
  same reference ids Java would generate; the gate is "Rust-written platform
  PELT decodes identically to Java-written platform PELT".
- **A Rust parser and compiler.** Independent of everything above; when it
  exists, it produces the same node shape and plugs into the same lowering.
  The archive branch holds a complete one; whether it is revived or rewritten
  is a decision for that day, not this document.

---

## 6. Integration matrix

| Existing technology | Step | Direction | Mechanism |
|---|---|---|---|
| `m3.pure` (M3 bootstrap) | 0 onward | in | embedded at compile time; chunk 0 of every model |
| Maven-built jars with PELT | 3–4 | in | PELT store over directory / jars / classloader |
| Java Pure compiler | all | upstream | unchanged; its PELT output is the contract |
| PCT corpus and adapters | 2 onward | test | "Rust interpreted" adapter in the fork and in legend-engine, registered red in Step 2; failures typed by kind |
| JNI | 2, 6 | both | `evaluate()`, value marshalling, handles, proxies, Java-object node source |
| legend-engine compiler (`toPureGraph`) | 6, 7 | in | live `CoreInstance` objects consumed through the node bridge; identity unification by path |
| `MetadataPelt` (class in `legend-pure-runtime-java-engine-compiled`, used by legend-engine's `PureModel`) | 6 | in | its lazy objects are also `CoreInstance`s; unified with the evaluator's own PELT-loaded elements by path |
| Java natives (`core_functions_*`) | 1 onward | re-implement | Rust native registry, fished per test (Step 1) then per the adapter's `FunctionNotFound` ranking (Step 5 onward) |

---

## 7. What this deliberately does not do

- It does not compile Pure. Repository code is compiled by Java; user code by
  legend-engine. A Rust compiler is a separate, later project that benefits from
  everything here but is not required by it.
- It does not carry the previous Rust workspace forward. Parser, compiler,
  LSP, DAP, MCP, DSL crates, CLI and the binary snapshot format stay on the
  archive branch. Anything needed later is fished, not merged.
- It does not replace either Java engine. They remain the oracle.
- It does not write to engine-built Java objects in the MVP.
- It does not attempt plan generation or store execution first. Those are Pure
  code with the largest native and extension surface; they sit at the end of the
  PCT-ranked native backlog, not its start.

---

## 8. Risks and how each step contains them

| Risk | Where it bites | Containment |
|---|---|---|
| Extraction leaves a hidden coupling behind (a field the evaluator reads that the old compiler filled, such as the mangled name on function nodes or the metatype lookup) | Steps 0–1 | Step 1's per-arm, per-element, per-dispatch-path tests over a bootstrap-only model surface every such contract before any loader exists |
| The executable form is lossy relative to M3 (missing node kinds, optional types) | Step 5 | property-bag fallback means nothing is unloadable; PCT histogram shows exactly which shapes are missing |
| Java-specific graph details (resolved import stubs, milestoning rewrites, inference results) | Step 5 | they are *data* the lowering reads, not behaviour to emulate; the Java compiler is upstream |
| Reference ids reach inside elements | Steps 4, 6 | forward references from bodies target elements plus `name`/`id` keys; sub-element ids only matter for back references, which are optional |
| JNI latency per node | Step 6 | one transition per object, cached; whole-model work uses bulk calls or a PELT snapshot of the model |
| Engine-built elements lack source spans | Step 6 | the node bridge tolerates missing spans; PELT snapshots would not, which is why live objects go through the bridge |
| A red-first adapter is noise | Step 2 | failures cross JNI with a kind; the report groups by kind, so "not loaded yet" never masquerades as a divergence, and the expected-failures manifest keeps CI green while the number climbs |
| Native coverage | Step 1 onward | measured per test, then per module by the adapter; target 100 %, reached incrementally |

---

## 9. Glossary

| Term | Meaning |
|---|---|
| M3 | The Pure metamodel: `Class`, `Function`, `Property`, `ValueSpecification`, … Every graph node is an instance of an M3 class. |
| Bootstrap | Chunk 0 of every model: primitive types, `Any`, `Nil`, and the M3 classes parsed from `m3.pure`. |
| `CoreInstance` | The Java interface every Pure graph node implements: classifier, name, source information, property access by name. |
| PELT | Pure Element serialization: one binary file per element plus per-module metadata, written by `compile-pure`. |
| Reference id | String path identifying a node across elements, stable across builds. |
| PCT | Platform Compatibility Testing: Pure functions and tests run on every engine; results must agree. |
| Adapter | The per-engine runner PCT uses to execute a test function. |
| Native | A Pure function declared without a body, implemented in the host language. |
| Mangled name | A function's identity string: simple name plus parameter and return types and multiplicities, e.g. `plus_Integer_MANY__Integer_1_`. The native registry and dispatch are keyed by it. |
| Lowering | Converting generic graph nodes into evaluator-specific structures. |
| Overlay | A per-request layer of definitions on top of the shared static graph. |
| Fishing | Copying one existing implementation from the archive into the workspace because a test or PCT failure needs it. |

---

## Appendix A — Extraction map (archive → new workspace)

All source paths are on the archive tag `archive/pre-evaluator-extract-2026-10`
(the old workspace root is `legend-pure-rust/`, crate names carried a
`legend-pure-parser-` / `legend-pure-` prefix). Copy, then apply the edit in
the third column. Nothing in these tables is rewritten from scratch. One edit
applies to every copied file: scrub doc comments that mention the snapshot
format, serialisation or serde (about 50 lines across the two crates), so the
grep gates stay honest.

**Table 1 — `crates/metamodel` (package `legend-pure-metamodel`).** Depends on
`smol_str`, `thiserror`, `rust_decimal`. About 6 k lines.

| Archive path | Destination | Edit on copy |
|---|---|---|
| `crates/ast/src/source_info.rs` (`SourceInfo` only) and `crates/ast/src/annotation.rs` (`SpannedString` only) | `src/source_info.rs` | Copy the two leaf types; do not depend on the AST crate. Keep `SourceInfo::new`. |
| `crates/pure/src/ids.rs` | `src/ids.rs` | Strip `Serialize`/`Deserialize`. Keep `ElementId`, `PackageId`, `RelationId`. |
| `crates/pure/src/arena.rs` | `src/arena.rs` | As is (fix the doc-test import path). |
| `crates/pure/src/types.rs` | `src/types.rs` | Strip serde. Keep **everything**: `TypeExpr`, `Multiplicity`, `Parameter`, `ResolvedType`, `ValueSpec`, `FunctionCallData`, `ExprKind` (all 20 arms, including the relation / column / path ones the evaluator stubs), `ConstValue`, `DateValue`, `PrimitiveType`, `RelationColumn*`, `PathStepLowered`, `ColSpecLiteralKind`. These are the language. |
| `crates/pure/src/annotations.rs` | `src/annotations.rs` | Strip serde. |
| `crates/pure/src/nodes/{class,function,enumeration,profile,association,measure,unit,mod}.rs` | `src/nodes/` | Strip serde. `profile.rs` uses `SpannedString` from `source_info.rs`. |
| `crates/pure/src/model.rs` | `src/model.rs` | Strip serde. Keep `ElementNode`, `Element` (all variants, `DSLInstance` included — Step 4 turns it into the property-bag element by replacing its opaque byte payload with a property map), `DSLInstance`, `Package`, `ModelChunk`, `DerivedIndexes`, and on `PureModel` the fields `global_packages`, `root_package`, `chunks`, `package_elements`, `derived`. **Drop** `extension_arenas`, `repo_visibility`, `repo_patterns`, `compile_scope` and the methods taking the AST `Package` type (`resolve_package`, `resolve_in_package`, `resolve_functions_by_name_in_package`). Keep the methods the evaluator calls (`get_node`, `get_element`, `try_get_element`, `element_name`, `get_package`, `resolve_fqn_str`, `resolve_by_path`, `resolve_function_by_path`, `association_properties`) and the ones the builders and the Step 4 lowering need (`new`, `get_or_create_package`, `register_element`, `rebuild_derived_indexes`, `specializations`, `derived`, `resolve_by_fqn`, `resolve_functions_by_simple_name_globally`). |
| `crates/pure/src/bootstrap.rs` | `src/bootstrap.rs` | As is (fix the doc-test import path, as in `arena.rs`). Keep the `include_str!` of `m3.pure` (relative path unchanged: the crate is at the same depth). Keep `create_bootstrap_chunk`, `metatype_of`, `register_m3_packages`, the `*_ID` constants. Add a `PureModel::bootstrapped()` convenience that does what the old `test_model()` did: `new()` + push the bootstrap chunk + register the M3 packages. |
| `crates/pure/src/m3_parser.rs` | `src/m3_parser.rs` | As is. |
| `crates/pure/src/fqn.rs` | `src/fqn.rs` | As is (`compute_function_fqn`: the mangling the builders and the Step 5 lowering need). |
| `crates/pure/src/resolve.rs` — only `least_upper_bound`, `least_upper_bound_ids`, `collect_ancestors`, `is_subtype`, `get_first_supertype` | `src/subtype.rs` | Extract the functions (≈ 100 lines). The rest of `resolve.rs` is the compiler. |
| `crates/pure/src/milestoning/mod.rs` | — | **Not in Step 0.** Fished in Step 5 together with the evaluator's milestoning dispatch if a PCT row needs it. |
| `crates/pure/src/{error,pipeline,infer,inference/,lower/,validate,extension,visibility,access,locate,island_lower,purem/}` | — | **Never.** Compiler, validator, binary snapshot format. `error.rs` is `CompilationError`; the evaluator's error type is in table 2. |

**Table 2 — `crates/evaluator` (package `legend-pure-evaluator`).** Depends on
`metamodel`, `smol_str`, `thiserror`, `rust_decimal`, `im-rc`, `jiff`,
`tracing`. About 8 k lines.

| Archive path | Destination | Edit on copy |
|---|---|---|
| `crates/runtime/src/error.rs` | `src/error.rs` | As is (`PureException`, `PureRuntimeError`). Add one variant, `Unsupported(&'static str)`, for the stubbed arms. |
| `crates/runtime/src/value.rs` | `src/value.rs` | As is. `FunctionValue::Path` stays as a type; nothing produces it until path literals are fished. |
| `crates/runtime/src/heap.rs` | `src/heap.rs` | As is (`HeapEntry::{Dynamic,Typed}`, `TypedObject`, `RuntimeHeap`). |
| `crates/runtime/src/context.rs`, `scope_guard.rs` | `src/context.rs`, `src/scope_guard.rs` | As is. |
| `crates/runtime/src/date.rs` | `src/date.rs` | As is. |
| `crates/runtime/src/m3_paths.rs` | `src/m3_paths.rs` | As is (classifier FQN constants the heap uses). |
| `crates/runtime/src/model_utils.rs` | `src/model_utils.rs` | As is (element path helpers; `heap.rs` calls them). |
| `crates/runtime/src/hooks.rs` | `src/hooks.rs` | Keep; the evaluator is generic over it. Drop the one hook method that takes a display tree (it is the only use of `display.rs`). |
| `crates/runtime/src/native.rs` | `src/native.rs` | Keep `Evaluated`, `EvalContextTrait`, `NativeFunction`, `NativeRegistry` (`new`, `register`, `get`, `find_by_prefix`, `get_or_err`), `expect_args`, `expect_min_args`, `force_all`, `force_thunk`, and the `lit_*` / `MockCtx` test helpers (they become the seed of the `builders` module). **Drop** `RuntimeExtension`, `RUNTIME_EXTENSIONS`, `with_extensions`, every `pub mod` of a native file, and the `extensions()` / `config_for()` trait methods. `NativeRegistry::standard()` registers nothing until Step 1 fishes natives. |
| `crates/runtime/src/eval.rs` | `src/eval.rs` | Keep `Evaluator`, `EvalContext`, `eval`, `eval_body`, `call`, `call_user_function*`, `eval_lambda_value`, `apply_callable`, `FoundQp`, `find_qp_with_generalization`, the free helpers at the bottom (`collect_free_variables`, `multiplicity_constant_bounds`). **Drop** the constructors `new_default_with_extensions*`, `new_default_with_dsl_populators`, `set_extension_configs`, `config_for`, `extensions`, and the display snapshot in `dispatch_snapshot_pause`. **Stub** with `Unsupported`: the `RelationLiteral`, `ColSpecArrayLiteral`, `ColSpecLiteral` and `PathLiteral` arms; the milestoning qualified-property hook. Move the 14 unit tests at the bottom to `tests/building_blocks.rs`. |
| `crates/runtime/src/builder.rs` | `src/builder.rs` | Keep `registry()` and `build()`; drop `populators` and `extension_configs`. |
| `crates/runtime/src/native/lang.rs` — `letFunction`, `if`, `new` (four keys), `copy` | `src/native/lang.rs` | **Step 1, one `impl NativeFunction` at a time.** `getAll*` and `dynamicNew*` stay behind until a test needs them. |
| `crates/runtime/src/native/{arithmetic,comparison,boolean}.rs` — `plus`, `minus`, `times`, `eq`, `equal`, `and`, `or`, `not` | `src/native/…` | **Step 1**, as the building-block tests reach them. |
| `crates/runtime/src/native/{collection,string,math,datetime,meta,equality}.rs` | — | **Step 5 onward**, per adapter `FunctionNotFound` row. |

**Table 3 — stays on the archive until the step that fishes it.**

| Archive path | Fished in | Note |
|---|---|---|
| `crates/runtime/src/{pct.rs,runner/}`, `crates/runtime/resources/pct_grammar_rust_native.json`, `crates/runtime/src/native/testing.rs` | Step 5, optional | Local Rust-side runner, exclusion manifest, report; `assert*` / `executePCTTest` natives (those are fished as soon as a PCT test needs them). The published numbers come from the Step 2 Java adapter. `testing.rs` reads the manifest through the old repository crate; replace that with a path. |
| `crates/runtime/src/relation.rs`, `src/native/relation/`, `crates/dsl-tds/src/csv.rs` | Step 5, when a PCT row needs relations | The CSV parser is copied as a module, not as a crate. |
| `crates/runtime/src/milestoning.rs` + `crates/pure/src/milestoning/mod.rs` | Step 5, when a PCT row needs milestoning | |
| `crates/runtime/src/dsl.rs` | Step 7 | DSL-instance hydration SPI. |
| `crates/runtime/src/{display,debug,extensions}.rs` | Not in the MVP | IDE debugger rendering; per-evaluator extension state. |
| `crates/jni/`, `legend-pure-runtime/legend-pure-runtime-rust-evaluator{,-bindings-ap}/` (Java, left in place but removed from the Maven reactor in Step 0), `crates/java-codegen/` | Step 2 | Handle table, value conversion, `PureRustEvaluator`, proxies. Cut the crate's dependencies on the parser, protocol, repository and codegen crates when fishing; re-add the two `<module>` lines to `legend-pure-runtime/pom.xml`. |
| `crates/runtime/tests/*.rs` (16 files) | Never as they are | Thirteen compile `.pure` source through the old platform loader; `discovery_smoke.rs` and the three `extension_*_smoke.rs` exercise the dropped extension SPI. Their *assertions* are a source of Step 1 and Step 5 test ideas. |
| `crates/{lexer,parser,ast,ast-derive,compose,protocol,ide,lsp,dap,mcp,cli,build,core-platform-pure,snapshot-builder,dsl-*,store-relational-runtime,stress,xtask}` | Never, by rule 1 | The Rust parser and compiler, tooling, DSLs, binary snapshot format. |

Rule for agents: if a step's "Build" list names something in these tables,
the task is to *fish* it (copy the smallest compiling unit and its test).
Writing a parallel implementation is a review failure; so is copying a whole
file when one function was asked for.

## Appendix B — Step 0 mechanics

**Archive first.**

```bash
# `legend-pure-rust` is both a branch and a directory; qualify the ref.
git tag archive/pre-evaluator-extract-2026-10 refs/heads/legend-pure-rust
git branch legend-pure-rust-archive refs/heads/legend-pure-rust
git push origin archive/pre-evaluator-extract-2026-10 legend-pure-rust-archive
```

**Reading from the archive without checking it out.**

```bash
git show archive/pre-evaluator-extract-2026-10:legend-pure-rust/crates/runtime/src/eval.rs > /tmp/eval.rs
git ls-tree -r --name-only archive/pre-evaluator-extract-2026-10 -- legend-pure-rust/crates/runtime/src/
```

**Workspace skeleton.**

```toml
# legend-pure-rust/Cargo.toml
[workspace]
members = ["crates/metamodel", "crates/evaluator"]
resolver = "2"

[workspace.package]
edition = "2024"
license = "Apache-2.0"

[workspace.dependencies]
smol_str = "0.3"
thiserror = "2"
rust_decimal = "1"
im-rc = "15"
jiff = { version = "0.2", features = ["std"] }
tracing = "0.1"
```

`crates/metamodel/Cargo.toml` takes `smol_str`, `thiserror`, `rust_decimal`.
`crates/evaluator/Cargo.toml` takes `legend-pure-metamodel = { path = "../metamodel" }`
and the remaining four. The versions above are the archive's, so copied code
compiles unchanged.

**Layout after Step 0.**

```
legend-pure-rust/
  Cargo.toml
  CLAUDE.md                 # build, test, the six rules, pointer to the plan
  README.md
  docs/rust-evaluator-mvp.md
  docs/pelt-analysis.md
  crates/metamodel/src/{lib,source_info,ids,arena,types,annotations,model,bootstrap,m3_parser,fqn,subtype}.rs
  crates/metamodel/src/nodes/
  crates/evaluator/src/{lib,error,value,heap,context,scope_guard,date,m3_paths,model_utils,hooks,native,eval,builder,builders}.rs
  crates/evaluator/src/native/            # empty until Step 1
  crates/evaluator/tests/building_blocks.rs
```

**Files outside the workspace that Step 0 touches.** `CLAUDE.md` at the
repository root (rewrite two sections), `.github/workflows/rust.yml` (new
command list), `legend-pure-runtime/pom.xml` (two `<module>` lines removed).
Nothing else in the Java tree changes.

**Gates, as commands.**

```bash
cargo build --workspace
cargo tree --workspace -e normal | grep -vE 'legend-pure-(metamodel|evaluator)|smol_str|thiserror|rust_decimal|im-rc|jiff|tracing' # transitive deps only
grep -rnE 'purem|pelt|legend_pure_parser|serde|linkme|postcard|snapshot' crates/   # expect: nothing
cargo test --workspace
```

## Appendix C — Open questions owned by the author

Implementers should not resolve these on their own.

1. **legend-engine target.** Which branch / commit of legend-engine is the
   target from Step 2 (adapter registration) onward — the fork's `rust-parser` branch or FINOS `upstream/master`?
   PELT contents and the set of `Test_*_PCT.java` adapters differ between them.
2. **Where Java-side helpers land.** The Step 2 adapter and `evaluate` entry
   point, the Step 3 classloader callback, the Step 4 oracle JUnit and the
   Step 6 node helper are Java code. Decision per
   item: fork branch only, or upstream PR to finos/legend-pure (the
   `legend-pure-runtime-rust-evaluator` module already lives in this fork).
3. **The engine PCT adapter.** Upstream PR to finos/legend-engine, or kept in
   the fork? This decides whether "Rust interpreted" results appear in FINOS CI
   or only locally.
4. **Fixture refresh cadence.** Who re-runs `regen-pelt-fixtures.sh` when the
   Java platform changes, and is a stale oracle a build failure or a warning?
5. **Extraction destination.** Step 0 assumes the new workspace *replaces*
   `legend-pure-rust/` on the live branch (archive keeps the old one). The
   alternative is a sibling directory with the old workspace left in place;
   that keeps two Rust directions visible, which rule 1 argues against.
   Default is replace unless the author says otherwise.
