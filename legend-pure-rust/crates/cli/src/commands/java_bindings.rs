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
//! # Usage
//!
//! ```bash
//! legend java-bindings \
//!     --output ./gen-java \
//!     --java-package com.example.gen \
//!     --functions meta::pure::functions::math::plus_Integer_MANY__Integer_1_
//! ```

use std::path::PathBuf;

use owo_colors::OwoColorize;
use smol_str::SmolStr;

use legend_pure_core_platform::repo::{self, Repo};
use legend_pure_java_codegen::{FqnInput, Options, generate};

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

    if requested.is_empty() {
        return Err(CliError::Custom(
            "no functions requested — pass --functions or --functions-file".to_owned(),
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

    let mut opts = Options::new(&args.java_package);
    if let Some(name) = args.functions_class {
        opts.functions_class_name = Some(name);
    }

    let files = generate(&model, &requested, &opts).map_err(|e| CliError::Custom(e.to_string()))?;

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
