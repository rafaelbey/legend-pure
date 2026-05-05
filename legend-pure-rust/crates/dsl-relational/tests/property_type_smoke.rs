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

//! Phase A5: per-property type-driven validators that need access to
//! the class hierarchy on `PureModel`.
//!
//! Mirrors the type-aware checks in Java's
//! `RelationalInstanceSetImplementationValidator.validatePropertyMappings`:
//! data-type-property-with-targetId, enum-property-needs-transformer,
//! class-typed-property-needs-join. Tests run through the full
//! compile pipeline (`compile_with_extensions`) so user classes are
//! actually loaded onto the model.

use indoc::indoc;
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_dsl_relational::compiler::RelationalExtension;
use legend_pure_dsl_relational::parser::{
    RelationalClassMappingBodyParser, RelationalSectionParser,
};
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::extension::CompilerExtension;

fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_sections(
        source,
        "property_type_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![
            Box::new(RelationalSectionParser),
            Box::new(MappingSectionParser::with_body_parsers(vec![Box::new(
                RelationalClassMappingBodyParser,
            )])),
        ],
    )
    .expect("source must parse")
}

fn compile_errors(source: &str) -> Vec<String> {
    let file = parse(source);
    let extensions: [Box<dyn CompilerExtension>; 2] = [
        Box::new(MappingExtension::new()),
        Box::new(RelationalExtension::new()),
    ];
    let exts: Vec<&dyn CompilerExtension> = extensions.iter().map(|e| e.as_ref()).collect();
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&[file], &[], &exts);
    match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors.iter().map(|e| e.message.clone()).collect(),
    }
}

// ---------------------------------------------------------------------------
// A5: data-type property must not carry `[targetId]`
// ---------------------------------------------------------------------------

#[test]
fn data_type_property_with_target_id_errors() {
    let errors = compile_errors(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeT (id INT PRIMARY KEY, name VARCHAR(50))
        )

        ###Pure
        Class pkg::Trade { id : Integer[1]; }

        ###Mapping
        Mapping pkg::TradeMap
        (
          pkg::Trade : Relational
          {
            ~mainTable [pkg::db]tradeT
            (id[srcA, tgtA] : tradeT.id)
          }
        )
    "});
    assert!(
        errors
            .iter()
            .any(|m| m.contains("returns a data type and thus should not have a targetId")),
        "expected data-type-with-targetId error; got {errors:#?}"
    );
}

#[test]
fn data_type_property_without_target_id_passes() {
    let errors = compile_errors(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeT (id INT PRIMARY KEY)
        )

        ###Pure
        Class pkg::Trade { id : Integer[1]; }

        ###Mapping
        Mapping pkg::TradeMap
        (
          pkg::Trade : Relational
          {
            ~mainTable [pkg::db]tradeT
            (id : tradeT.id)
          }
        )
    "});
    assert!(
        !errors.iter().any(|m| m.contains("data type")),
        "expected no data-type errors; got {errors:#?}"
    );
}

// ---------------------------------------------------------------------------
// A5: enum property requires EnumerationMapping transformer
// ---------------------------------------------------------------------------

#[test]
fn enum_property_without_transformer_errors() {
    let errors = compile_errors(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeT (id INT PRIMARY KEY, color VARCHAR(20))
        )

        ###Pure
        Enum pkg::Color { RED, GREEN, BLUE }
        Class pkg::Trade { id : Integer[1]; color : pkg::Color[1]; }

        ###Mapping
        Mapping pkg::TradeMap
        (
          pkg::Trade : Relational
          {
            ~mainTable [pkg::db]tradeT
            (
              id : tradeT.id,
              color : tradeT.color
            )
          }
        )
    "});
    assert!(
        errors
            .iter()
            .any(|m| m.contains("enum") && m.contains("EnumerationMapping")),
        "expected enum-without-transformer error; got {errors:#?}"
    );
}

#[test]
fn enum_property_with_transformer_passes() {
    let errors = compile_errors(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeT (id INT PRIMARY KEY, color VARCHAR(20))
        )

        ###Pure
        Enum pkg::Color { RED, GREEN, BLUE }
        Class pkg::Trade { id : Integer[1]; color : pkg::Color[1]; }

        ###Mapping
        Mapping pkg::TradeMap
        (
          pkg::Color : EnumerationMapping ColorMap
          {
            RED : ['R'],
            GREEN : ['G'],
            BLUE : ['B']
          }

          pkg::Trade : Relational
          {
            ~mainTable [pkg::db]tradeT
            (
              id : tradeT.id,
              color : EnumerationMapping ColorMap : tradeT.color
            )
          }
        )
    "});
    assert!(
        !errors
            .iter()
            .any(|m| m.contains("enum") && m.contains("EnumerationMapping")),
        "expected no enum-without-transformer error; got {errors:#?}"
    );
}

// ---------------------------------------------------------------------------
// A5: class-typed property must be mapped to a join
// ---------------------------------------------------------------------------

#[test]
fn class_typed_property_without_join_errors() {
    let errors = compile_errors(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeT (id INT PRIMARY KEY, prodId INT)
          Table prodT (id INT PRIMARY KEY)
        )

        ###Pure
        Class pkg::Product { id : Integer[1]; }
        Class pkg::Trade
        {
          id : Integer[1];
          product : pkg::Product[1];
        }

        ###Mapping
        Mapping pkg::TradeMap
        (
          pkg::Product[prodMap] : Relational
          {
            ~mainTable [pkg::db]prodT
            (id : prodT.id)
          }

          pkg::Trade[tradeMap] : Relational
          {
            ~mainTable [pkg::db]tradeT
            (
              id : tradeT.id,
              product : tradeT.prodId
            )
          }
        )
    "});
    assert!(
        errors
            .iter()
            .any(|m| m.contains("class") && m.contains("not a join")
                || m.contains("relationalOperation is not a join")),
        "expected class-without-join error; got {errors:#?}"
    );
}

#[test]
fn class_typed_property_with_join_passes() {
    let errors = compile_errors(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeT (id INT PRIMARY KEY, prodId INT)
          Table prodT (id INT PRIMARY KEY)
          Join tradeProd (tradeT.prodId = prodT.id)
        )

        ###Pure
        Class pkg::Product { id : Integer[1]; }
        Class pkg::Trade
        {
          id : Integer[1];
          product : pkg::Product[1];
        }

        ###Mapping
        Mapping pkg::TradeMap
        (
          pkg::Product[prodMap] : Relational
          {
            ~mainTable [pkg::db]prodT
            (id : prodT.id)
          }

          pkg::Trade[tradeMap] : Relational
          {
            ~mainTable [pkg::db]tradeT
            (
              id : tradeT.id,
              product[prodMap] : [pkg::db]@tradeProd | prodT.id
            )
          }
        )
    "});
    assert!(
        !errors.iter().any(|m| m.contains("not a join")),
        "expected no class-without-join error; got {errors:#?}"
    );
}
