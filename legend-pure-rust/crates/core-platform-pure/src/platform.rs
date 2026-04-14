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

//! Platform source loading and compilation.
//!
//! Provides [`load_platform()`] to parse and compile the standard library
//! into a [`PureModel`]. The same [`parse_and_compile()`] function can be
//! used by any Pure repository — it is not platform-specific.
//!
//! # Loading Strategy
//!
//! Currently, `load_platform()` always parses from embedded `.pure` source
//! files and compiles on the fly (`from-source`). A future `from-binary`
//! feature flag will load a pre-compiled `.purem` (`FlatBuffers`) artifact
//! instead, bypassing the parser and compiler entirely.
//!
//! ```text
//! ┌─────────────────────────────────────────────────┐
//! │             load_platform()                     │
//! │                                                 │
//! │  #[cfg(feature = "from-source")]                │
//! │  ┌──────────────────────────┐                   │
//! │  │ .pure → parse → compile │ → PureModel       │
//! │  └──────────────────────────┘                   │
//! │                                                 │
//! │  #[cfg(feature = "from-binary")]                │
//! │  ┌──────────────────────────┐                   │
//! │  │ .purem → deserialize    │ → PureModel       │
//! │  └──────────────────────────┘                   │
//! └─────────────────────────────────────────────────┘
//! ```
//!
//! The `from-binary` path will be the default for release builds; the
//! `from-source` path remains for development iteration.

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::pipeline::{self, PartialPureModel};
use smol_str::SmolStr;

use crate::sources;

/// The auto-imports for platform Pure code.
/// Only includes packages that exist from the Pure files we actually compile.
pub const PLATFORM_AUTO_IMPORTS: &[&str] = &[
    "meta::pure::functions::lang",
    "meta::pure::functions::boolean",
    "meta::pure::functions::collection",
    "meta::pure::functions::math",
    "meta::pure::functions::string",
    "meta::pure::functions::date",
    "meta::pure::functions::meta",
    "meta::pure::functions::asserts",
    "meta::pure::functions::io",
    "meta::pure::functions::tools",
    "meta::pure::profiles",
    "meta::pure::test::pct",
    "meta::pure::test::surveyor",
];

/// Load all platform Pure sources: parse → compile → return model.
///
/// Returns `Ok(PureModel)` if everything parsed and compiled cleanly, or
/// `Err(PartialPureModel)` if any parse or compilation errors occurred.
/// The partial model still contains all successfully resolved elements.
///
/// # Errors
///
/// Returns `Err(PartialPureModel)` with all parse and compilation errors.
/// Callers must inspect `errors` to decide whether to proceed.
#[allow(clippy::result_large_err)]
pub fn load_platform() -> Result<PureModel, PartialPureModel> {
    let raw_sources = sources::platform_sources();

    let auto_imports: Vec<SmolStr> = PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect();

    parse_and_compile(
        raw_sources.iter().map(|s| (s.content, s.path)),
        &auto_imports,
    )
}

/// Parse and compile any set of Pure sources.
///
/// This is the generic entry point for compiling Pure code from any
/// repository — platform, user code, or test fixtures.
///
/// `sources` is an iterator of `(content, name)` pairs.
///
/// Parse errors are converted to [`CompilationError`]s with
/// [`CompilationErrorKind::ParseFailure`] so all errors are reported
/// uniformly in the `PartialPureModel`.
///
/// # Errors
///
/// Returns `Err(PartialPureModel)` if any parse or compilation errors occur.
#[allow(clippy::result_large_err)]
pub fn parse_and_compile<'a>(
    sources: impl Iterator<Item = (&'a str, &'a str)>,
    auto_imports: &[SmolStr],
) -> Result<PureModel, PartialPureModel> {
    let mut parsed_files = Vec::new();
    let mut parse_errors: Vec<CompilationError> = Vec::new();

    for (content, name) in sources {
        match legend_pure_parser_parser::parse(content, name) {
            Ok(src_file) => parsed_files.push(src_file),
            Err(partial) => {
                // Recover valid elements
                parsed_files.push(partial.source_file);
                // Convert parse errors to compilation errors
                for e in partial.errors {
                    let source_info = e
                        .source_info()
                        .cloned()
                        .unwrap_or_else(|| SourceInfo::new(name, 0, 0, 0, 0));
                    parse_errors.push(CompilationError {
                        message: e.message(),
                        source_info,
                        kind: CompilationErrorKind::ParseFailure {
                            source: SmolStr::new(name),
                        },
                    });
                }
            }
        }
    }

    match pipeline::compile(&parsed_files, auto_imports) {
        Ok(model) if parse_errors.is_empty() => Ok(model),
        Ok(model) => Err(PartialPureModel {
            model,
            errors: parse_errors,
        }),
        Err(mut partial) => {
            // Prepend parse errors before compilation errors
            parse_errors.append(&mut partial.errors);
            partial.errors = parse_errors;
            Err(partial)
        }
    }
}
