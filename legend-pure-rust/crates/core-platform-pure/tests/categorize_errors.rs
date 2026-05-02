// Copyright 2026 Goldman Sachs
// Licensed under the Apache License, Version 2.0

//! Scratch test: Categorize platform compilation errors (detailed).

use legend_pure_core_platform::platform::load_platform;
use std::collections::HashMap;

#[test]
fn categorize_platform_errors_detailed() {
    match load_platform() {
        Ok(_) => println!("Platform compiled cleanly!"),
        Err(partial) => {
            let total = partial.errors.len();
            println!("Total errors: {total}");

            let mut by_kind: HashMap<String, usize> = HashMap::new();
            let mut unresolved_names: HashMap<String, usize> = HashMap::new();
            let mut duplicate_names: HashMap<String, usize> = HashMap::new();
            let mut ambiguous_names: HashMap<String, usize> = HashMap::new();
            let mut unsupported: HashMap<String, usize> = HashMap::new();

            for e in &partial.errors {
                let kind_str = match &e.kind {
                    legend_pure_parser_pure::error::CompilationErrorKind::UnresolvedElement { .. } => "UnresolvedElement",
                    legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { .. } => "DuplicateElement",
                    legend_pure_parser_pure::error::CompilationErrorKind::CyclicInheritance { .. } => "CyclicInheritance",
                    legend_pure_parser_pure::error::CompilationErrorKind::InvalidAssociation { .. } => "InvalidAssociation",
                    legend_pure_parser_pure::error::CompilationErrorKind::InvalidSuperType { .. } => "InvalidSuperType",
                    legend_pure_parser_pure::error::CompilationErrorKind::InvalidAnnotation { .. } => "InvalidAnnotation",
                    legend_pure_parser_pure::error::CompilationErrorKind::DuplicateProperty { .. } => "DuplicateProperty",
                    legend_pure_parser_pure::error::CompilationErrorKind::DuplicateVariable { .. } => "DuplicateVariable",
                    legend_pure_parser_pure::error::CompilationErrorKind::ParseFailure { .. } => "ParseFailure",
                    legend_pure_parser_pure::error::CompilationErrorKind::AmbiguousImport { .. } => "AmbiguousImport",
                    legend_pure_parser_pure::error::CompilationErrorKind::UnsupportedExpression { .. } => "UnsupportedExpression",
                    legend_pure_parser_pure::error::CompilationErrorKind::UnknownProperty { .. } => "UnknownProperty",
                    legend_pure_parser_pure::error::CompilationErrorKind::QualifiedPropertyArityMismatch { .. } => "QualifiedPropertyArityMismatch",
                    legend_pure_parser_pure::error::CompilationErrorKind::QualifiedPropertyArgTypeMismatch { .. } => "QualifiedPropertyArgTypeMismatch",
                    legend_pure_parser_pure::error::CompilationErrorKind::CannotInferLambdaParameterTypes { .. } => "CannotInferLambdaParameterTypes",
                    legend_pure_parser_pure::error::CompilationErrorKind::NotVisible { .. } => "NotVisible",
                };
                *by_kind.entry(kind_str.to_string()).or_default() += 1;

                match &e.kind {
                    legend_pure_parser_pure::error::CompilationErrorKind::UnresolvedElement { path } => {
                        *unresolved_names.entry(path.to_string()).or_default() += 1;
                    }
                    legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { name } => {
                        *duplicate_names.entry(name.to_string()).or_default() += 1;
                    }
                    legend_pure_parser_pure::error::CompilationErrorKind::AmbiguousImport { name, candidates } => {
                        *ambiguous_names.entry(format!("{name} -> {candidates:?}")).or_default() += 1;
                    }
                    legend_pure_parser_pure::error::CompilationErrorKind::UnsupportedExpression { kind } => {
                        *unsupported.entry(kind.to_string()).or_default() += 1;
                    }
                    legend_pure_parser_pure::error::CompilationErrorKind::ParseFailure { source } => {
                        println!("  PARSE FAILURE: {} @ {:?}", source, e.source_info);
                    }
                    _ => {}
                }
            }

            println!("\n=== By Error Kind ===");
            let mut kinds: Vec<_> = by_kind.into_iter().collect();
            kinds.sort_by(|a, b| b.1.cmp(&a.1));
            for (kind, count) in &kinds {
                println!("  {count:5}  {kind}");
            }

            println!("\n=== Top 30 Unresolved Names ===");
            let mut names: Vec<_> = unresolved_names.into_iter().collect();
            names.sort_by(|a, b| b.1.cmp(&a.1));
            for (name, count) in names.iter().take(30) {
                println!("  {count:5}  {name}");
            }

            println!("\n=== Top 20 DuplicateElement Names ===");
            let mut dupes: Vec<_> = duplicate_names.into_iter().collect();
            dupes.sort_by(|a, b| b.1.cmp(&a.1));
            for (name, count) in dupes.iter().take(20) {
                println!("  {count:5}  {name}");
            }

            println!("\n=== AmbiguousImport Names ===");
            let mut ambs: Vec<_> = ambiguous_names.into_iter().collect();
            ambs.sort_by(|a, b| b.1.cmp(&a.1));
            for (name, count) in ambs.iter().take(20) {
                println!("  {count:5}  {name}");
            }

            println!("\n=== UnsupportedExpression kinds ===");
            let mut uns: Vec<_> = unsupported.into_iter().collect();
            uns.sort_by(|a, b| b.1.cmp(&a.1));
            for (kind, count) in uns.iter().take(20) {
                println!("  {count:5}  {kind}");
            }

            println!("\nTotal unique unresolved names: {}", names.len());
            println!("Total unique duplicate names: {}", dupes.len());
        }
    }
}
