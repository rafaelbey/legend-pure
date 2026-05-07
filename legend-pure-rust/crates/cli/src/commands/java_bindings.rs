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

//! `legend java-bindings` — Generate Java wrappers around the JNI evaluator.
//!
//! For each requested Pure function FQN, emits typed Java static-method
//! facades plus interfaces for every reachable user-defined class. The
//! generated sources target the JNI runtime support library at
//! `org.finos.legend.pure.rust.proxy`.
//!
//! Reachability is seeded from the requested functions' parameter and
//! return types. Use `--classes` and `--associations` to add more seeds
//! for elements that aren't referenced by any wrapper function — e.g.
//! when generating bindings for a model the user will populate via
//! `evaluate(...)` returning generic values, or for an association whose
//! endpoint classes wouldn't otherwise be pulled in.
//!
//! For Maven-style "bootstrap an evaluator from a manifest" workflows,
//! `--bindings-file <PATH>` accepts a single file containing one FQN per
//! line; the CLI resolves each entry against the model and dispatches
//! it to the right slot (function / class / association) based on its
//! actual element kind. Blank lines and `#`-prefixed comments are
//! ignored.
//!
//! # Usage
//!
//! ```bash
//! legend java-bindings \
//!     --output ./gen-java \
//!     --java-package com.example.gen \
//!     --functions meta::pure::functions::math::plus_Integer_MANY__Integer_1_ \
//!     --classes user_test::Person \
//!     --associations user_test::PersonAddress
//!
//! # Or, bootstrap from a curated list:
//! legend java-bindings \
//!     --output target/generated-sources/java-bindings \
//!     --java-package org.finos.legend.pure.rust.generated \
//!     --bindings-file src/main/pure-bindings/m3-bindings.txt
//! ```

use std::path::PathBuf;

use owo_colors::OwoColorize;
use smol_str::SmolStr;

use legend_pure_core_platform::repo::{self, Repo};
use legend_pure_java_codegen::{FqnInput, Options, generate};
use legend_pure_parser_pure::model::{Element, PureModel};

use crate::diagnostics::CliError;

/// Arguments for the `legend java-bindings` command.
#[derive(clap::Args)]
pub struct JavaBindingsArgs {
    /// Output directory. Each generated `.java` file is written under
    /// the directory layout that matches its Java package.
    #[arg(short, long, value_name = "DIR")]
    pub output: PathBuf,

    /// Java root package — every emitted class lives under this prefix.
    #[arg(long, value_name = "PKG", default_value = "com.example.legend.gen")]
    pub java_package: String,

    /// Mangled FQN of a Pure function to wrap. Repeat for multiple
    /// functions. Combine with `--functions-file` to provide a list.
    #[arg(long = "functions", value_name = "FQN", action = clap::ArgAction::Append)]
    pub functions: Vec<String>,

    /// File of FQNs (one per line, blank lines and `#`-prefixed comments
    /// ignored).
    #[arg(long, value_name = "FILE")]
    pub functions_file: Option<PathBuf>,

    /// FQN of a Pure `Class` to emit a Java interface for, beyond what
    /// is reachable from `--functions`. Repeatable.
    #[arg(long = "classes", value_name = "FQN", action = clap::ArgAction::Append)]
    pub classes: Vec<String>,

    /// FQN of a Pure `Association` to seed the reachability walk with.
    /// Both participating classes are added to the seed set so the
    /// association-injected properties surface on their generated
    /// interfaces. Repeatable.
    #[arg(long = "associations", value_name = "FQN", action = clap::ArgAction::Append)]
    pub associations: Vec<String>,

    /// Single file listing FQNs of Pure elements to bind, one per line.
    /// The CLI resolves each entry and dispatches it by element kind
    /// (Function / Class / Association). Blank lines and `#`-prefixed
    /// comments are ignored. Suitable for Maven-style bootstrap
    /// manifests that mix functions, classes, and associations.
    #[arg(long, value_name = "FILE")]
    pub bindings_file: Option<PathBuf>,

    /// Simple class name for the generated static-functions facade.
    /// Defaults to `PureFunctions`.
    #[arg(long, value_name = "NAME")]
    pub functions_class: Option<String>,
}

/// Execute the `legend java-bindings` command.
#[allow(clippy::needless_pass_by_value)]
pub fn run(args: JavaBindingsArgs) -> Result<(), CliError> {
    eprintln!(
        "{} Java bindings into {}...",
        "Generating".cyan().bold(),
        args.output.display()
    );

    let mut requested: Vec<FqnInput> = args
        .functions
        .iter()
        .map(|s| FqnInput::new(s.trim()))
        .filter(|f| !f.raw.is_empty())
        .collect();

    if let Some(file) = args.functions_file.as_deref() {
        let content = std::fs::read_to_string(file).map_err(|e| CliError::Io {
            path: file.to_path_buf(),
            source: e,
        })?;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            requested.push(FqnInput::new(trimmed));
        }
    }

    let mut extra_classes: Vec<FqnInput> = args
        .classes
        .iter()
        .map(|s| FqnInput::new(s.trim()))
        .filter(|f| !f.raw.is_empty())
        .collect();
    let mut extra_associations: Vec<FqnInput> = args
        .associations
        .iter()
        .map(|s| FqnInput::new(s.trim()))
        .filter(|f| !f.raw.is_empty())
        .collect();

    // Read the kind-agnostic bindings file (one FQN per line) into a
    // raw vector; we'll dispatch each entry by element kind once the
    // model is loaded.
    let bindings_lines = if let Some(file) = args.bindings_file.as_deref() {
        let content = std::fs::read_to_string(file).map_err(|e| CliError::Io {
            path: file.to_path_buf(),
            source: e,
        })?;
        content
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    if requested.is_empty()
        && extra_classes.is_empty()
        && extra_associations.is_empty()
        && bindings_lines.is_empty()
    {
        return Err(CliError::Custom(
            "nothing to generate — pass --functions, --functions-file, --classes, \
             --associations, or --bindings-file"
                .to_owned(),
        ));
    }

    eprintln!("  {} loading platform model...", "•".dimmed());
    let repos = Repo::default_with_build_snapshots();
    let auto_imports: Vec<SmolStr> = legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect();
    let model = match repo::load(&repos, &auto_imports) {
        Ok(m) => m,
        Err(partial) => {
            eprintln!(
                "{} platform model loaded with {} compile error(s); continuing",
                "warning:".yellow().bold(),
                partial.errors.len()
            );
            partial.model
        }
    };

    // Dispatch every entry from the bindings file by its element kind.
    if !bindings_lines.is_empty() {
        let (fns_added, classes_added, assocs_added) = dispatch_bindings_by_kind(
            &model,
            &bindings_lines,
            &mut requested,
            &mut extra_classes,
            &mut extra_associations,
        )?;
        eprintln!(
            "  {} bindings file: {} function(s), {} class(es), {} association(s)",
            "•".dimmed(),
            fns_added,
            classes_added,
            assocs_added,
        );
    }

    let mut opts = Options::new(&args.java_package);
    if let Some(name) = args.functions_class {
        opts.functions_class_name = Some(name);
    }

    let files = generate(
        &model,
        &requested,
        &extra_classes,
        &extra_associations,
        &opts,
    )
    .map_err(|e| CliError::Custom(e.to_string()))?;

    let mut written = 0usize;
    for file in &files {
        let dest = args.output.join(&file.relative_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CliError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        std::fs::write(&dest, &file.contents).map_err(|e| CliError::Io {
            path: dest.clone(),
            source: e,
        })?;
        written += 1;
    }

    eprintln!(
        "  {} wrote {} Java source file(s) under {}",
        "✓".green(),
        written.to_string().bold(),
        args.output.display()
    );
    Ok(())
}

/// Resolve each entry in a `--bindings-file` and append it to the right
/// slot based on the element kind found in the model.
///
/// Returns `(functions_added, classes_added, associations_added)` for
/// the per-kind summary.
fn dispatch_bindings_by_kind(
    model: &PureModel,
    lines: &[String],
    fns_out: &mut Vec<FqnInput>,
    classes_out: &mut Vec<FqnInput>,
    associations_out: &mut Vec<FqnInput>,
) -> Result<(usize, usize, usize), CliError> {
    let mut fns_count = 0usize;
    let mut classes_count = 0usize;
    let mut assocs_count = 0usize;

    for raw in lines {
        let id = match model.resolve_fqn_str(raw) {
            Some(id) => id,
            None => {
                return Err(CliError::Custom(format!(
                    "bindings-file entry `{raw}` could not be resolved in the model"
                )));
            }
        };
        match model.get_element(id) {
            Element::Function(_) => {
                fns_out.push(FqnInput::new(raw));
                fns_count += 1;
            }
            Element::Class(_) => {
                classes_out.push(FqnInput::new(raw));
                classes_count += 1;
            }
            Element::Association(_) => {
                associations_out.push(FqnInput::new(raw));
                assocs_count += 1;
            }
            other => {
                return Err(CliError::Custom(format!(
                    "bindings-file entry `{raw}` resolved to an unsupported element kind ({}) \
                     — only Function, Class, and Association are supported",
                    element_kind(other)
                )));
            }
        }
    }
    Ok((fns_count, classes_count, assocs_count))
}

fn element_kind(e: &Element) -> &'static str {
    match e {
        Element::Class(_) => "Class",
        Element::Enumeration(_) => "Enumeration",
        Element::Function(_) => "Function",
        Element::Profile(_) => "Profile",
        Element::Association(_) => "Association",
        Element::Measure(_) => "Measure",
        Element::PrimitiveType(_) => "PrimitiveType",
        Element::Unit(_) => "Unit",
        Element::PackageableMultiplicity(_) => "Multiplicity",
        Element::Package(_) => "Package",
    }
}
