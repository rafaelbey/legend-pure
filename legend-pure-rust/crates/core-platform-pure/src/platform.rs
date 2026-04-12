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

//! Parse and compile orchestration.

use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::pipeline;
use smol_str::SmolStr;

use crate::sources;

/// The compiled platform model plus metadata.
#[derive(Debug)]
pub struct PlatformModel {
    /// The compiled model containing all platform elements.
    pub model: PureModel,
    /// Compilation errors encountered.
    pub compilation_errors: Vec<CompilationError>,
}

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
#[must_use]
pub fn load_platform() -> PlatformModel {
    let raw_sources = sources::platform_sources();
    let mut parsed_files = Vec::new();

    // 1. Parsing
    for source in raw_sources {
        // Ignoring specific file parse errors within load_platform itself,
        // relying on compilation to pick them up or letting caller decide
        if let Ok(src_file) = legend_pure_parser_parser::parse(source.content, source.path) {
            parsed_files.push(src_file);
        } else {
            // Ignore parse errors, the model will just be missing those elements
            // In a stricter scenario we'd panic or record these.
        }
    }

    // Convert auto imports to smol string
    let auto_imports: Vec<SmolStr> = PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect();

    // 2. Compilation
    match pipeline::compile(&parsed_files, &auto_imports) {
        Ok(model) => PlatformModel {
            model,
            compilation_errors: Vec::new(),
        },
        Err(partial) => PlatformModel {
            model: partial.model,
            compilation_errors: partial.errors,
        },
    }
}
