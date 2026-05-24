// Copyright 2026 Goldman Sachs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Worked example of a downstream Legend Pure extension.
//!
//! Demonstrates the six self-registering extension surfaces — copy
//! this directory as a template, replace each unit-struct's stub
//! impl with your DSL's real logic, ship a binary. Companion
//! `mydsl-legend` binary lives at `src/main.rs`; it's a one-line
//! wrapper around `legend_cli::main()` plus the force-link `use`
//! statements that keep the linker from dropping the distributed-
//! slice statics below.

use legend_pure_ide::{IDE_EXTENSIONS, IdeExtension, Reference};
use legend_pure_parser_ast::dsl::DSLElement;
use legend_pure_parser_ast::island::IslandContent;
use legend_pure_parser_parser::ParserContext;
use legend_pure_parser_parser::error::ParseError;
use legend_pure_parser_parser::island::{ISLAND_PARSERS, IslandParser};
use legend_pure_parser_parser::section_parser::{SECTION_PARSERS, SectionParser};
use legend_pure_parser_pure::extension::{
    COMPILER_EXTENSIONS, CompilerExtension, DeclareCtx, DefineCtx, ValidateCtx,
};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::types::ValueSpec;
use legend_pure_runtime::dsl::{DSL_POPULATORS, DSLPopulationCtx, DSLPopulator};
use legend_pure_runtime::error::PureException;
use legend_pure_runtime::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, RUNTIME_EXTENSIONS,
    RuntimeExtension, expect_args, force_all,
};
use legend_pure_runtime::value::Value;
use linkme::distributed_slice;

// ---------------------------------------------------------------------------
// 1. RuntimeExtension — ships one native, `mydsl::greet(String[1]): String[1]`
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct GreetNative;

impl NativeFunction for GreetNative {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("mydsl::greet", args, 1)?;
        let forced = force_all(args, ctx)?;
        let name = forced[0].as_string()?;
        Ok(Evaluated::new(Value::String(
            format!("hello, {name}").into(),
        )))
    }
}

/// Native-function provider — self-registered via the
/// [`RUNTIME_EXTENSIONS`] distributed slice. Picked up by
/// [`NativeRegistry::discovered`] and by `Evaluator::builder().build(&model)`
/// in any binary that link-forces this crate.
#[derive(Debug, Default)]
pub struct MyDslRuntimeExtension;

impl RuntimeExtension for MyDslRuntimeExtension {
    fn name(&self) -> &'static str {
        "mydsl-runtime"
    }
    fn register_natives(&self, r: &mut NativeRegistry) {
        r.register("greet_String_1__String_1_", GreetNative);
    }
}

#[distributed_slice(RUNTIME_EXTENSIONS)]
static MY_DSL_RUNTIME_EXTENSION: &(dyn RuntimeExtension + Sync) = &MyDslRuntimeExtension;

// ---------------------------------------------------------------------------
// 2. SectionParser — handles `###MyDsl` blocks. Stub body for the example.
// ---------------------------------------------------------------------------

/// Parses the body of a `###MyDsl` section. Real downstream consumers
/// would consume tokens from `ctx.cursor()` here and build their own
/// `DSLElement` payloads; this stub returns an empty Vec so any
/// `###MyDsl` section is silently absorbed.
#[derive(Debug, Default)]
pub struct MyDslSectionParser;

impl SectionParser for MyDslSectionParser {
    fn kind(&self) -> &'static str {
        "MyDsl"
    }
    fn parse_body(
        &self,
        _ctx: &mut ParserContext<'_>,
        _errors: &mut Vec<ParseError>,
    ) -> Vec<Box<dyn DSLElement>> {
        Vec::new()
    }
}

#[distributed_slice(SECTION_PARSERS)]
static MY_DSL_SECTION_PARSER: &(dyn SectionParser + Send + Sync) = &MyDslSectionParser;

// ---------------------------------------------------------------------------
// 3. IslandParser — handles `#mytag{ … }#` inline grammars. Stub.
// ---------------------------------------------------------------------------

/// Parses an inline `#mytag{ … }#` island. Real consumers would
/// consume tokens and return a typed `IslandContent`; this stub
/// errors so any `#mytag{}#` use surfaces an obvious diagnostic.
#[derive(Debug, Default)]
pub struct MyDslIslandParser;

impl IslandParser for MyDslIslandParser {
    fn tag(&self) -> &str {
        "mytag"
    }
    fn parse(&self, ctx: &mut ParserContext<'_>) -> Result<Box<dyn IslandContent>, ParseError> {
        // Stub: surface an "Unexpected" parser error pointing at the
        // current cursor token. Real consumers consume tokens via
        // `ctx.cursor()` and return a typed `IslandContent`.
        let si = ctx.cursor().current_source_info().clone();
        Err(ParseError::Unexpected {
            message: "mydsl-extension example: #mytag{…}# parser is a stub — replace this body \
                      with real island parsing for your DSL"
                .to_string(),
            source_info: si,
        })
    }
}

#[distributed_slice(ISLAND_PARSERS)]
static MY_DSL_ISLAND_PARSER: &(dyn IslandParser + Send + Sync) = &MyDslIslandParser;

// ---------------------------------------------------------------------------
// 4. CompilerExtension — declare / define / validate hooks. Stub no-ops.
// ---------------------------------------------------------------------------

/// Hooks into the compile pipeline. Real DSLs use the declare hook
/// to push `Element::DSLInstance` rows for each parsed section
/// element (see `crates/dsl-mapping/src/compiler.rs` for the
/// canonical pattern); this stub is a no-op so the example doesn't
/// touch the model.
#[derive(Debug, Default)]
pub struct MyDslCompilerExtension;

impl CompilerExtension for MyDslCompilerExtension {
    fn name(&self) -> &'static str {
        "mydsl-compiler"
    }
    fn declare(&self, _ctx: &mut DeclareCtx<'_>) {}
    fn define_signatures(&self, _ctx: &mut DefineCtx<'_>) {}
    fn define_bodies(&self, _ctx: &mut DefineCtx<'_>) {}
    fn validate(&self, _ctx: &mut ValidateCtx<'_>) {}
}

#[distributed_slice(COMPILER_EXTENSIONS)]
static MY_DSL_COMPILER_EXTENSION: &(dyn CompilerExtension + Sync) = &MyDslCompilerExtension;

// ---------------------------------------------------------------------------
// 5. DSLPopulator — heap-hydrates Element::DSLInstance rows. Stub.
// ---------------------------------------------------------------------------

/// Projects this DSL's compile-time data (carried as
/// `Element::DSLInstance` rows whose `dsl_name == "MyDsl"`) into
/// heap property values. Real consumers decode the snapshot bytes
/// and call `ctx.heap.mutate_set(...)` on the pre-allocated handle.
/// This stub does nothing because the section parser above doesn't
/// emit any elements yet.
#[derive(Debug, Default)]
pub struct MyDslPopulator;

impl DSLPopulator for MyDslPopulator {
    fn dsl_name(&self) -> &'static str {
        "MyDsl"
    }
    fn populate(&self, _ctx: DSLPopulationCtx<'_>) {}
}

#[distributed_slice(DSL_POPULATORS)]
static MY_DSL_POPULATOR: &(dyn DSLPopulator + Sync) = &MyDslPopulator;

// ---------------------------------------------------------------------------
// 6. IdeExtension — contributes IDE reference sites. Stub.
// ---------------------------------------------------------------------------

/// Surfaces clickable reference regions in this DSL's source spans
/// to the IDE's `ReferenceIndex`. Real consumers iterate their
/// `Element::DSLInstance` payloads on the model and push one
/// `Reference` per source-level reference site. This stub emits
/// nothing — there are no elements to walk in the example.
#[derive(Debug, Default)]
pub struct MyDslIdeExtension;

impl IdeExtension for MyDslIdeExtension {
    fn name(&self) -> &'static str {
        "mydsl-ide"
    }
    fn walk_references(&self, _model: &PureModel, _visit: &mut dyn FnMut(Reference)) {}
}

#[distributed_slice(IDE_EXTENSIONS)]
static MY_DSL_IDE_EXTENSION: &dyn IdeExtension = &MyDslIdeExtension;

// ---------------------------------------------------------------------------
// Convenience re-exports for the binary's force-link `use`s.
// ---------------------------------------------------------------------------

/// One-stop import for downstream binaries — pulling these symbols
/// in (e.g. `use legend_pure_mydsl_extension::*;`) is enough to
/// force the linker to keep this crate's object file, which is what
/// guarantees the `#[distributed_slice]` statics above actually
/// reach `RUNTIME_EXTENSIONS`, `COMPILER_EXTENSIONS`, etc.
pub mod prelude {
    pub use crate::{
        MyDslCompilerExtension, MyDslIdeExtension, MyDslIslandParser, MyDslPopulator,
        MyDslRuntimeExtension, MyDslSectionParser,
    };
}
