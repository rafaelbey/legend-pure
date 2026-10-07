# A Pure Evaluator in Rust — MVP Proposal

> **Status: proposal, October 2026.** This document is self-contained. It assumes
> the reader knows Legend Pure and legend-engine as they exist in Java today and
> nothing else. It describes what a minimum viable Rust evaluator is, which
> existing technologies it plugs into, and a sequence of steps where each step
> ships something usable and proves something specific.

---

## 0. Orientation for a fresh session

Read this first; everything below assumes it.

| What | Where |
|---|---|
| This document | `legend-pure-rust/docs/rust-evaluator-mvp.md` on branch `legend-pure-rust` of the `legend-pure` fork (`rafaelbey/legend-pure`). It is the single source of truth for the Rust direction. |
| The Rust workspace | `legend-pure-rust/` (Cargo workspace, edition 2024). Build and test: `cargo build --workspace`, `cargo nextest run --workspace` (or `cargo test --workspace`), lint gates `cargo lint-lib` and `cargo lint`, copyright check `./scripts/check-copyright.sh`. See `legend-pure-rust/CLAUDE.md` for conventions. |
| The Java stack (upstream FINOS code) | Sibling directories at the repository root: `legend-pure-core/`, `legend-pure-runtime/`, `legend-pure-maven/`, … Requires JDK 11 or 17 and Maven. |
| PELT files for the platform, locally | `legend-pure-core/legend-pure-m3-core/target/classes/**/*.pelt` plus `org/finos/legend/pure/module/platform.{pmf,psr,pxr,pbr,pfn}`. Produced by `mvn -T 4 install -DskipTests -pl legend-pure-core/legend-pure-m3-core -am` (15–30 min first time). ~1,900 files, 3.5 MB. |
| PELT byte-level reference | `docs/architecture/pelt-serialization.md` at the repository root — on branch `docs/pelt-format`, submitted upstream as finos/legend-pure PR #1360. **Not on `legend-pure-rust` yet**: `git cherry-pick c989b50953b` (that single commit) before Step 1. Do **not** merge the branch — it tracks upstream `master` and differs from `legend-pure-rust` by ~700 files. |
| Background analysis (why PELT, what it contains, how it maps) | `legend-pure-rust/docs/deferred/pelt_vs_purem.md` — to be moved to `legend-pure-rust/docs/` in Step 0. Sections 6.3–6.8 are the source of Steps 3–6 here. |
| legend-engine | Checkout at `~/Projects/legend-engine` (currently on branch `rust-parser`; `upstream/master` is the FINOS main line — see Appendix C for which is the Step 5 target). Its `*-pure` modules emit PELT into their jars (e.g. 414 `.pelt` in `functions-standard`); its PCT adapters are the `Test_*_PCT.java` classes (93 on `upstream/master`); the engine compiler is `legend-engine-language-pure-compiler` (`toPureGraph`); the Pure function libraries are `legend-engine-core/legend-engine-core-pure/legend-engine-pure-code-functions-{standard,relation,json,planExecution,…}`. |
| PCT in the platform | Profile `meta::pure::test::pct::PCT` in `legend-pure-core/legend-pure-m3-core/src/main/resources/platform/pure/essential/tests/pct_core.pure`; ~470 `<<PCT.test>>` functions in the platform sources; Java report providers `Essential_Functions_PCTReportProvider`, `Grammar_Functions_PCTReportProvider`, `Core_{Compiled,Interpreted}_PCTReportProvider`; shared helpers in `m3/pct/shared/PCTTools.java`. |
| Existing Rust code that the steps build on | Appendix A. Read it before starting any step so nothing is built twice. |
| Step 0 concrete inventory | Appendix B. The only place the superseded format is named. |
| Decisions still owned by the author | Appendix C. Do not guess these; ask. |

**Definition of done for the MVP.** Step 5's gate: a "Rust interpreted" PCT
adapter registered in legend-engine, reporting pass rates for the platform and
at least one `legend-engine-pure-code-functions-*` module, with a published
ranked list of missing natives. Step 6 is the first increment after the MVP.

---

## 1. Thesis

Legend already has a complete Pure **compiler** in Java, and legend-engine
already has its own compiler that turns user models (protocol JSON) into a Pure
graph. What a Rust effort can add first, with the least risk, is an
**evaluator**: a component that executes Pure functions.

Three facts make an evaluator-first approach possible:

1. **The Java build already writes the compiled graph to disk in an open,
   element-granular binary format called PELT.** Every `*-pure` module in
   legend-pure and legend-engine emits it. It contains function bodies, not just
   signatures, so a program can be executed from it without any `.pure` source
   and without re-compiling.
2. **PELT, and every Java Pure object, share one shape:** a node is a
   classifier (the Pure class it is an instance of) plus a bag of named property
   values. An evaluator that understands that shape can consume graphs built by
   the Java compiler, by legend-engine's compiler, or by anything else, through
   the same code.
3. **PCT (Platform Compatibility Testing) is an existing, executable
   definition of "behaves like Java".** Pure functions tagged `<<PCT.function>>`
   and their `<<PCT.test>>` tests run on both Java engines on every build. A
   third engine that runs the same tests gets a per-function parity score for
   free.

So the MVP is: **read PELT, execute what is in it, exchange values with Java
over JNI, and measure itself with PCT.** No Rust parser or compiler is needed
for any step below. Natives (functions declared `native` in Pure and implemented
in Java) must be re-implemented in Rust; the target is 100 %, and PCT turns that
into a ranked, incremental backlog rather than a precondition.

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
        PELT --> RE[Rust evaluator]
        UG -. JNI node bridge .-> RE
        RE -. JNI values / proxies .-> JAVA[Java callers]
        PCT[PCT suites] --> RE
    end
```

| Technology | What it is | How the evaluator uses it |
|---|---|---|
| **PELT** | Per-element binary files (`<path>.pelt`) plus per-module metadata (`.pmf` manifest, `.psr` sources, `.pxr` external refs, `.pbr` back references, `.pfn` function names), produced by the `compile-pure` Maven goal into `target/classes` and therefore into every jar. Byte-level reference: `docs/architecture/pelt-serialization.md` in the legend-pure repository. | The source of all *static* code: platform functions, engine core functions, DSL metamodels. Read-only input. |
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
- **Reference id.** A string identity for a node across elements: the element's
  path for an element, otherwise the element path plus a navigation path, e.g.
  `meta::pure::metamodel::type::Class.properties['name']` or
  `my::f__String_1_.expressionSequence[0].parametersValues[1]`. Stable across
  builds.
- **Static vs dynamic.** *Static* is everything compiled at build time and
  shipped as PELT. *Dynamic* is what legend-engine builds per request from user
  input: classes, mappings, queries, lambdas, as live Java objects.
- **Lowering.** Turning the generic node shape into whatever the evaluator
  executes. For declarations this is building type and property tables; for
  function bodies this is building an executable expression tree.
- **Native.** A Pure function declared without a body. Each needs a Rust
  implementation.

---

## 4. The MVP in one picture

```mermaid
flowchart TD
    subgraph Inputs
        STORE[PELT store\ndirectory · jar list · classloader]
        JOBJ[Java objects\nengine-built graph]
    end
    subgraph Rust evaluator
        DIR[Element directory\nfrom manifests: path → classifier, span, module\nfunctions-by-name · back references]
        NODES[Node source\none interface, two implementations:\nPELT bytes · JNI-backed Java object]
        LOW[Lowering\ndeclarations → type tables\nbodies → executable expression trees\ncached per element / per Java object]
        EVAL[Evaluator\nvalues · variables · lambdas · dispatch\nmemoised, lazy by element]
        NAT[Native registry\nRust implementations of native functions]
    end
    subgraph Boundary
        JNI[JNI layer\nevaluate() · values · handles · proxies]
        PCTA[PCT adapter\n"Rust interpreted"]
    end
    STORE --> DIR --> NODES
    JOBJ --> NODES
    NODES --> LOW --> EVAL
    NAT --> EVAL
    EVAL <--> JNI
    PCTA --> JNI
```

Design rules that hold for every step:

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

### Step 0 — Clean the workspace so only one direction is live

**Goal.** Make sure anyone (or any agent) opening the workspace finds exactly
one plan — this document — and no code or doc that argues for a superseded
one. History and an archive branch keep everything recoverable.

**Scope decision (2026-10-07): remove the superseded decision only.** The
binary snapshot format the workspace previously used as its own compiled-graph
artifact is replaced by PELT and goes away, together with its pipeline and
docs. The parser, compiler, LSP, MCP, DAP and DSL crates stay as existing
assets; they are documented as *off the MVP critical path*, not deleted.

**Build.**

1. *Freeze.* Tag the current head `archive/pre-pelt-2026-10` and create branch
   `legend-pure-rust-archive` at the same commit. Nothing is lost.
2. *Docs first, one commit.* Rewrite `CLAUDE.md`, `ARCHITECTURE.md`,
   `README.md`, `BACKLOG.md` (12 superseded rows) and `TODO.md` to this
   document's direction; delete the snapshot-format design docs and the
   deferred CLI docs (`docs/deferred/cli_*.md`,
   `docs/deferred/lazy_loading_binary_format.md`, the per-crate format doc);
   prune the six `docs/runtime/*` files and `.cargo/config.toml` of
   snapshot-format passages; move the PELT analysis out of `docs/deferred/`.
   After this commit no document contradicts the plan, even before any code
   moves.
3. *Code removal.* Delete the snapshot format module, the `snapshot-builder`
   crate, the build-script shapes that compile repositories into snapshots on
   every cold build, the snapshot `Repo` variant and classpath kind, the
   `legend snapshot` CLI command, the `xtask` release staging of snapshots, and
   the ~74 test files that exercise them (references in 26 crates, ~1.4 k
   lines). **Target shape after removal:** all nine repositories declared in
   `crates/core-platform-pure/Cargo.toml` switch to `shape = "embedded"`, the
   build crate's original `include_str!` shape, which already embeds each
   repository's `.pure` sources *and* its `.json` manifests (PCT exclusion
   lists) as one file list, surfaced through `Repo::sources()` /
   `Repo::manifests()`. Nothing is lost; cold start compiles from sources
   until the PELT reader (steps 1–2) takes over as the binary load path.
   Verify this on one DSL repository before deleting anything.
4. *Agent memory.* Project memory entries that describe the snapshot format are
   deleted or rewritten so future sessions start from this document. (Outside
   the repository; not part of the gate.)

**Gate.** `cargo test --workspace` green with the snapshot code gone; the
grep in Appendix B returns only Appendix B itself; `CLAUDE.md` names
`docs/rust-evaluator-mvp.md` as the single source of truth.

**Start here.** Appendix B lists every path and the grep to confirm nothing is
left. Rough size: docs commit half a day; code removal two to three days.

**Why before Step 1.** Step 1 adds a *second* binary graph format to the
workspace. If the first one is still live at that point, every reader has to
work out which one is current. Removing it first keeps the question from ever
arising.

### Step 1 — Read PELT

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
- A command-line `inspect` that dumps one element and `ls` that lists a
  module's elements from its manifest.

**Integrates with.** The jars Maven already builds. Nothing on the Java side
changes.

**Proves.** The format is fully understood. Gate: golden tests decoding every
file in the platform's `target/classes` without error and round-tripping the
string table; `inspect` output reviewed against the Java `print` of the same
element.

**Start here.** New leaf crate `crates/pelt` (dependencies: `flate2`, a zip
reader only). Fixtures: copy a dozen representative files from the platform
`target/classes` (a class, an enumeration, a profile, an association, a native
function, a test function with a lambda, `Root.pelt`, `Integer.pelt`, the five
`platform.*` module files) into `crates/pelt/tests/fixtures/`. The Java `print`
of an element comes from a throwaway JUnit test in `legend-pure-m3-core` using
`PureCompilerLoader` on `target/classes`. Rough size: ~2 k lines.

### Step 2 — A static graph in memory

**Goal.** Load modules by name, resolve dependencies, and answer "what is `X`"
for every element without executing anything.

**Build.**
- Element directory from manifests: path → classifier, source span, module;
  follow manifest dependencies transitively (as `MetadataPelt` does).
- Declarative lowering: classes (properties, supertypes, type parameters),
  enumerations, profiles, associations, primitive types, function *signatures*.
  Produces type and property tables the evaluator will use for `instanceOf`,
  property typing and dispatch.
- Overload index from `.pfn` (simple name → functions); subtype and
  association-inverse indexes from `.pbr`, so no element has to be loaded to
  answer "who extends `X`".
- Packages: real ones from their PELT files, virtual ones implied by element
  paths.
- A generic "property-bag element" for any classifier the lowering does not
  know (DSL metamodels, engine extensions), so *every* element is loadable.

**Integrates with.** Module dependencies and repository boundaries exactly as
Java sees them; a `kind = pelt` entry in a classpath description pointing at a
directory, jar or Maven coordinate.

**Proves.** Type-level questions match Java. Gate: for every element in the
platform manifest, classifier, supertypes, property names and types and
function signatures agree with what the Java `CoreInstance` API reports for the
same element.

**Start here.** Build the oracle first: a JUnit test in `legend-pure-m3-core`
that loads `target/classes` through `PureCompilerLoader` and dumps, per
element, `(path, classifier, supertypes, properties[(name, type, multiplicity)],
function signature)` as JSON into `crates/pelt/tests/fixtures/platform-oracle.json`.
The Rust gate is then a diff against that file. The lowering target is the
existing model types (Appendix A), not new ones. Rough size: 2–3 k lines.

**Keeping fixtures honest.** Step 1 fixtures and the oracle both derive from
`target/classes`, which only exists after a Maven build. Add
`legend-pure-rust/scripts/regen-pelt-fixtures.sh` that runs the Maven module
build, runs the oracle test, and copies the chosen files into
`crates/pelt/tests/fixtures/`; commit the fixtures and the script, and record
the legend-pure commit they were generated from in a `FIXTURES.md` next to
them. The oracle JUnit is committed on the fork branch under
`legend-pure-core/legend-pure-m3-core/src/test/java/.../pelt/` (a test, not
product code); whether it goes upstream is an open question (Appendix C).

### Step 3 — Execute function bodies, measure with PCT

**Goal.** Run Pure functions whose bodies come from PELT.

**Build.**
- Body lowering: M3 expression nodes (`SimpleFunctionExpression`,
  `InstanceValue`, `VariableExpression`, `LambdaFunction`, `KeyExpression`,
  type references) into an executable expression tree. Calls are already
  resolved to their target function by the Java compiler; types and
  multiplicities are already inferred on every expression. The lowering reads
  them, it does not recompute them.
- The evaluator: values (primitives, dates, decimals, collections,
  instances), variable scopes, lambdas with captures, dispatch on the resolved
  target, `let`, `if`, `match`, `new`, `copy`, property access and qualified
  properties, exceptions with source spans. Lazy per element: a function body is
  lowered on first call.
- The native registry: Rust implementations keyed by the function's mangled
  name, invoked when the resolved target is a `native function`.
- A PCT runner: find `<<PCT.test>>` functions in the loaded modules, execute
  each, classify failures as *missing native* or *lowering gap*, and print a
  histogram.

**Integrates with.** The PCT corpus as it exists in the platform `.pure`
sources, compiled by Java and read from PELT. No test is rewritten.

**Proves.** Semantic parity, function by function. Gate: a published pass
rate on the platform PCT suite with a ranked list of missing natives; the
rate only goes up.

**How PCT works, in one paragraph.** A PCT function is a platform function
carrying `<<PCT.function>>`; its tests are functions carrying `<<PCT.test>>`
(477 in the platform today) that call the function and assert on the result.
An *adapter* is a Pure function that decides how a test's expression is
executed on a given engine; each engine registers its adapter and a report
provider. A test passes when evaluating it raises no assertion. Per-engine
exclusions (known gaps) are declared in a manifest so the pass rate is honest
about what is skipped.

**Start here.** The evaluator, native registry (~200 registrations today),
PCT helpers, runner and exclusion manifest already exist (Appendix A); this
step changes *where bodies come from* — PELT instead of the existing source
path — and adds the missing-native / lowering-gap classification to the
runner's report (`crates/runtime/src/runner/report.rs`; `pct.rs` only holds
manifest / adapter / exclusion helpers). Rough size of the body lowering:
3–5 k lines.

**One model per evaluator instance.** An evaluator is built over exactly one
loaded model. In this step that model is PELT-loaded; the existing
source-compiled platform remains available for Rust-authored code and tooling,
but the two are never mixed inside one evaluator, so dispatch has a single
owner. "Same test, source-compiled vs PELT-loaded" is run as two evaluator
instances.

**How natives become incremental.** The histogram is the backlog. Each native
is weighted by how many PCT functions it unblocks, so the order of work is
data-driven, and "100 % natives" is reached one measured step at a time
rather than demanded up front.

### Step 4 — Values across JNI

**Goal.** Let Java call Pure functions in the Rust evaluator and get results
back.

**Build.**
- A Java class with `evaluate(functionPath, args...)` backed by a native
  method; a context object holding the loaded graph and the evaluator.
- Marshalling: Java primitives, strings, dates and decimals to and from Rust
  values; collections both ways.
- Handles for Rust-side instances: Java holds an opaque identifier, Rust keeps
  the object alive until Java releases it. Optional typed Java proxies
  generated per Pure class so callers see `person.firstName()` rather than
  `getProperty("firstName")`.
- The PELT store implementation that reads bytes through the JVM's own
  classloader, so the evaluator inside a JVM uses the jars already on the
  classpath and copies nothing.

**Integrates with.** Any JVM process that has Pure jars on its classpath;
legend-engine in particular.

**Proves.** The evaluator is usable as a library. Gate: the PCT runner from
step 3 re-run through the JNI entry point from Java with identical results;
a load test showing no handle leaks.

**Start here.** The JNI crate and its Java module
(`legend-pure-runtime/legend-pure-runtime-rust-evaluator`: `PureRustEvaluator`,
`PureRustInstance`, proxies) already exist (Appendix A). New in this step: the
classloader-backed PELT store (Java side: a `ClassLoader.getResource` callback
exposed to native code) and a `--classpath kind=pelt` entry in the existing
classpath description. Rough size: under 1 k lines Rust, ~300 lines Java.

### Step 5 — Consume legend-engine's graph live

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
  such as `my::Person`), fetch the class node, run the step-2 declarative
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
results equal to the Java compiled engine's; then a "Rust interpreted" PCT
adapter registered in legend-engine, reporting per-module pass rates.

**Start here.** Model the adapter on an existing one: any
`Test_*_PCT.java` in `legend-engine-core/legend-engine-core-pure/legend-engine-pure-code-functions-standard`
shows the registration shape. The Java node helper lives next to
`PureRustEvaluator`; it needs only the `CoreInstance` interface
(`legend-pure-m4`), so it works for handcrafted, PELT-lazy and interpreted
objects alike. First target: a function whose inputs are classes and
properties only, before anything that touches mappings. Rough size: ~2 k
lines Rust (node bridge, identity unification, overlay), ~400 lines Java
(node helper, adapter).

### Step 6 — DSL instances and property bags end to end

**Goal.** Mappings, runtimes, connections, services and other DSL instances
from the engine flow through the evaluator into the static Pure code that
consumes them (routing, analytics, in-memory execution).

**Build.**
- Generic hydration of property-bag elements into evaluator instances, with
  the schema read from the DSL's metamodel class (itself static, from PELT):
  property names, types and multiplicities validate the bag.
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

---

## 6. Integration matrix

| Existing technology | Step | Direction | Mechanism |
|---|---|---|---|
| Maven-built jars with PELT | 1–2 | in | PELT store over directory / jars / classloader |
| Java Pure compiler | all | upstream | unchanged; its PELT output is the contract |
| PCT corpus and adapters | 3, 5 | test | Rust PCT runner; "Rust interpreted" adapter in legend-engine |
| JNI | 4, 5 | both | `evaluate()`, value marshalling, handles, proxies, Java-object node source |
| legend-engine compiler (`toPureGraph`) | 5, 6 | in | live `CoreInstance` objects consumed through the node bridge; identity unification by path |
| `MetadataPelt` (class in `legend-pure-runtime-java-engine-compiled`, used by legend-engine's `PureModel`) | 5 | in | its lazy objects are also `CoreInstance`s; unified with the evaluator's own PELT-loaded elements by path |
| Java natives (`core_functions_*`) | 3 onward | re-implement | Rust native registry, prioritised by PCT histogram |

---

## 7. What this deliberately does not do

- It does not compile Pure. Repository code is compiled by Java; user code by
  legend-engine. A Rust compiler is a separate, later project that benefits from
  everything here but is not required by it.
- It does not replace either Java engine. They remain the oracle.
- It does not write to engine-built Java objects in the MVP.
- It does not attempt plan generation or store execution first. Those are Pure
  code with the largest native and extension surface; they sit at the end of the
  PCT-ranked native backlog, not its start.

---

## 8. Risks and how each step contains them

| Risk | Where it bites | Containment |
|---|---|---|
| The executable form is lossy relative to M3 (missing node kinds, optional types) | step 3 | property-bag fallback means nothing is unloadable; PCT histogram shows exactly which shapes are missing |
| Java-specific graph details (resolved import stubs, milestoning rewrites, inference results) | step 3 | they are *data* the lowering reads, not behaviour to emulate; the Java compiler is upstream |
| Reference ids reach inside elements | steps 2, 5 | forward references from bodies target elements plus `name`/`id` keys; sub-element ids only matter for back references, which are optional |
| JNI latency per node | step 5 | one transition per object, cached; whole-model work uses bulk calls or a PELT snapshot of the model |
| Engine-built elements lack source spans | step 5 | the node bridge tolerates missing spans; PELT snapshots would not, which is why live objects go through the bridge |
| Native coverage | step 3 onward | measured per module by PCT; target 100 %, reached incrementally |

---

## 9. Glossary

| Term | Meaning |
|---|---|
| M3 | The Pure metamodel: `Class`, `Function`, `Property`, `ValueSpecification`, … Every graph node is an instance of an M3 class. |
| `CoreInstance` | The Java interface every Pure graph node implements: classifier, name, source information, property access by name. |
| PELT | Pure Element serialization: one binary file per element plus per-module metadata, written by `compile-pure`. |
| Reference id | String path identifying a node across elements, stable across builds. |
| PCT | Platform Compatibility Testing: Pure functions and tests run on every engine; results must agree. |
| Adapter | The per-engine runner PCT uses to execute a test function. |
| Native | A Pure function declared without a body, implemented in the host language. |
| Lowering | Converting generic graph nodes into evaluator-specific structures. |
| Overlay | A per-request layer of definitions on top of the shared static graph. |

---

## Appendix A — Existing Rust code the steps build on (implementation notes)

The narrative above is written for readers who know nothing about prior Rust
work. The workspace, however, already contains a large amount of it. These are
the pieces each step **reuses rather than rewrites**; read the crate `README`
or `ARCHITECTURE.md` before touching it.

| Existing piece | Where | Used by | Notes |
|---|---|---|---|
| Model types the evaluator executes: element arena, class / function / property tables, the expression IR | `crates/pure/src/{model.rs,types.rs,nodes/}` | Steps 2–3 | These are the **lowering targets**. PELT nodes are lowered *into* them; do not invent a second model. The Rust parser and compiler that also populate them (`crates/lexer`, `crates/parser`, `crates/pure/src/pipeline.rs`, `lower/`, `resolve.rs`, `infer.rs`, `inference/`) stay but are off the MVP path. |
| Evaluator: values, heap, variable scopes, lambdas, dispatch, memoization | `crates/runtime/src/{eval.rs,value.rs,heap.rs,…}` | Steps 3–6 | Already runs the platform PCT subset from the source path; `crates/runtime/ARCHITECTURE.md`. |
| Native registry | `crates/runtime/src/native/` (~200 registrations) | Step 3 onward | Keyed by mangled function name; the PCT histogram drives what gets added. |
| PCT helpers, runner, report + exclusion manifest | `crates/runtime/src/pct.rs` (manifest / adapter / exclusion helpers), `crates/runtime/src/runner/` (execution, `report.rs`), platform-side `surveyor.pure` for discovery, `crates/runtime/resources/pct_grammar_rust_native.json`, `crates/runtime/tests/pct_coverage_audit.rs` | Step 3 | Extend `runner/report.rs` with the missing-native / lowering-gap split. |
| Repository loading, classpath description (`legend-pure-classpath.toml`), embedded platform sources | `crates/core-platform-pure/src/{repo.rs,classpath.rs}`, `crates/cli/src/classpath.rs` | Steps 2, 4 | Add the PELT repository kind here; the embedded-source kind remains as fallback. |
| JNI layer + Java module | `crates/jni`, `legend-pure-runtime/legend-pure-runtime-rust-evaluator` (`PureRustEvaluator`, `PureRustInstance`, `proxy/`) | Steps 4–5 | `evaluate()`, handle table, Java proxies already exist; add the classloader store and the node bridge. |
| Typed Java proxy generation | `crates/java-codegen`, `legend java-bindings` | Step 4 (optional) | Generates per-class Java interfaces over Rust handles. |
| Heap objects with a classifier + property bag, DSL instance hydration | `crates/runtime/src/{heap.rs,dsl.rs}` | Steps 5–6 | The Java-backed node and the generic property-bag hydration slot in here. |
| CLI | `crates/cli` (`legend …`) | Steps 1, 4 | `legend pelt inspect | ls | diff` go here. |

Rule for agents: if a step's "Build" list names something that exists in this
table, the task is to *extend* it. Creating a parallel implementation is a
review failure.

## Appendix B — Step 0 inventory (the only place the superseded format is named)

The superseded format is `.purem`. Everything below is deleted or rewritten in
Step 0; the archive tag keeps it recoverable.

**Code to delete**

- `crates/pure/src/purem/` (whole module; `lib.rs` export), `crates/snapshot-builder/`
  (and its entry in the root `Cargo.toml` `members`).
- `crates/build/src/lib.rs`: the `purem-embedded` / `purem-artifact` shapes and
  `emit_purem_*` functions; `crates/core-platform-pure/build.rs` snapshot
  compilation; the `LEGEND_PURE_SKIP_DSL_SNAPSHOTS` escape hatch.
- `crates/core-platform-pure/src/repo.rs`: `Repo::Purem`, `from_purem_*`,
  `purem_blob`, the `manifests` side table; `classpath.rs`: `kind = "purem"`.
- `crates/cli/src/commands/snapshot.rs` and its wiring; `crates/xtask` snapshot
  staging (`cargo dist`); `.cargo/config.toml` comments.
- Tests: `crates/*/tests/purem_roundtrip.rs`, `crates/core-platform-pure/tests/{purem_platform_smoke,purem_load_e2e,classpath_bytes_e2e,repo_parity}.rs`,
  `crates/snapshot-builder/tests/*`, and the ~70 other files the grep below lists.
- Examples: `examples/mydsl-extension/src/lib.rs`, `examples/README.md`.

**Docs to delete**

- `crates/core-platform-pure/docs/PUREM_FORMAT.md`
- `docs/deferred/lazy_loading_binary_format.md`, `docs/deferred/cli_plan.md`,
  `docs/deferred/cli_package.md`, `docs/deferred/cli_publish.md`

**Docs to rewrite** (remove the format, point at this document): `CLAUDE.md`,
`ARCHITECTURE.md`, `README.md`, `BACKLOG.md` (rows mentioning the format),
`TODO.md`, `docs/CODE_REVIEW.md`, `docs/PURE_LANGUAGE_SPEC.md`,
`docs/extensions/downstream-recipe.md`, `docs/runtime/{README,convergence_analysis,architecture_deep_questions,hybrid_compilation,error_location_design,perf_session_2026-05-01}.md`.
Move `docs/deferred/pelt_vs_purem.md` to `docs/pelt-analysis.md`.

**Grep that must come back empty except for this appendix**

```bash
grep -rli 'purem' legend-pure-rust --include='*.rs' --include='*.toml' --include='*.md' \
  --exclude-dir=target | grep -v 'docs/rust-evaluator-mvp.md'
```

**Archive**

```bash
# `legend-pure-rust` is both a branch and a directory; qualify the ref.
git tag archive/pre-pelt-2026-10 refs/heads/legend-pure-rust
git branch legend-pure-rust-archive refs/heads/legend-pure-rust
git push origin archive/pre-pelt-2026-10 legend-pure-rust-archive
```

## Appendix C — Open questions owned by the author

Implementers should not resolve these on their own.

1. **legend-engine target.** Which branch / commit of legend-engine is the
   Step 5 target — the fork's `rust-parser` branch or FINOS `upstream/master`?
   PELT contents and the set of `Test_*_PCT.java` adapters differ between them.
2. **Where Java-side helpers land.** The Step 2 oracle JUnit, the Step 4
   classloader callback and the Step 5 node helper are Java code. Decision per
   item: fork branch only, or upstream PR to finos/legend-pure (the
   `legend-pure-runtime-rust-evaluator` module already lives in this fork).
3. **The engine PCT adapter.** Upstream PR to finos/legend-engine, or kept in
   the fork? This decides whether "Rust interpreted" results appear in FINOS CI
   or only locally.
4. **Fixture refresh cadence.** Who re-runs `regen-pelt-fixtures.sh` when the
   Java platform changes, and is a stale oracle a build failure or a warning?
