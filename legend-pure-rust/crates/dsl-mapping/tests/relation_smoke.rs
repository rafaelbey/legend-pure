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

//! Stage-8 `RelationFunctionClassMapping` validator coverage (T2.2 c2).
//!
//! Java parity (see commit message of T2.2 c2 for the full audit):
//!  1. `~func` FQN must resolve to a Function element.
//!  2. Each non-local property mapping's name must exist on the target
//!     class (with supertype walk). Local-property mappings (`+name :
//!     Type[m] : col`) declare a new property and are exempt.
//!  3. Each `Binding pkg::B :` transformer's binding FQN must resolve.

use indoc::indoc;
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::extension::CompilerExtension;

fn parse(name: &str, source: &str) -> SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        name,
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::new())],
    );
    match result {
        Ok(f) => f,
        Err(p) => panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        ),
    }
}

fn compile(sources: Vec<SourceFile>) -> Vec<CompilationError> {
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&sources, &[], &exts);
    match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors,
    }
}

// ---------------------------------------------------------------------------
// 1. `~func` FQN resolution
// ---------------------------------------------------------------------------

#[test]
fn relation_with_known_function_validates_clean() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        function my::test::myRelFn() : meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>[1]
        {
          ^meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>()
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Relation
          {
            ~func my::test::myRelFn():meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>[1]
            legalName : name_col
          }
        )
    "};
    let file = parse("relation_clean.pure", source);
    let errors = compile(vec![file]);
    // Filter to relation-validator-specific messages. Other errors
    // (e.g. unresolved `Relation` because the relation metamodel isn't
    // loaded in this test scaffold) are not the c2 validator's
    // concern.
    let relation_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("RelationFunction")
                || (e.message.contains("Class")
                    && e.message.contains("has no property"))
        })
        .collect();
    assert!(
        relation_errors.is_empty(),
        "no RelationFunction-validator errors expected; got: {:#?}",
        relation_errors
            .iter()
            .map(|e| &e.message)
            .collect::<Vec<_>>()
    );
}

#[test]
fn relation_with_unresolved_function_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Relation
          {
            ~func my::test::missing__doesNotExist():meta::pure::metamodel::relation::Relation<Any>[1]
            legalName : name_col
          }
        )
    "};
    let file = parse("relation_missing_fn.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors.iter().any(|e| e.message.contains("RelationFunction `~func`")
            && e.message.contains("missing__doesNotExist")),
        "expected unresolved-function error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 2. Property existence on target class
// ---------------------------------------------------------------------------

#[test]
fn relation_with_unknown_property_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        function my::test::myRelFn() : meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>[1]
        {
          ^meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>()
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Relation
          {
            ~func my::test::myRelFn():meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>[1]
            notAProperty : whatever_col
          }
        )
    "};
    let file = parse("relation_unknown_prop.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors.iter().any(|e| {
            e.message.contains("Class 'my::test::Firm'")
                && e.message.contains("notAProperty")
        }),
        "expected unknown-property error citing notAProperty; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn relation_with_inherited_property_validates_clean() {
    // Property declared on supertype must satisfy the existence rule —
    // walk the `super_types` chain.
    let source = indoc! {r"
        ###Pure
        Class my::test::Named
        {
          name : String[1];
        }

        Class my::test::Firm extends my::test::Named
        {
        }

        function my::test::myRelFn() : meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>[1]
        {
          ^meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>()
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Relation
          {
            ~func my::test::myRelFn():meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>[1]
            name : name_col
          }
        )
    "};
    let file = parse("relation_inherited_prop.pure", source);
    let errors = compile(vec![file]);
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("Class") && e.message.contains("has no property 'name'")),
        "inherited property `name` must satisfy the existence rule; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn relation_with_local_property_declaration_is_exempt_from_existence_check() {
    // `+computed : String[1] : col` DECLARES a new property on the
    // mapping — Java's `LocalMappingPropertyInfo` treats it as not
    // requiring class-side existence. Validator must not flag.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        function my::test::myRelFn() : meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>[1]
        {
          ^meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>()
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Relation
          {
            ~func my::test::myRelFn():meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>[1]
            +brandNewLocalProp : String[1] : derived_col
          }
        )
    "};
    let file = parse("relation_local_prop.pure", source);
    let errors = compile(vec![file]);
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("has no property 'brandNewLocalProp'")),
        "+local property must not trigger the existence rule; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 3. Binding transformer FQN resolution
// ---------------------------------------------------------------------------

#[test]
fn relation_with_unresolved_binding_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        function my::test::myRelFn() : meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>[1]
        {
          ^meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>()
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Relation
          {
            ~func my::test::myRelFn():meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>[1]
            legalName : Binding my::test::missingBinding : name_col
          }
        )
    "};
    let file = parse("relation_missing_binding.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors.iter().any(|e| {
            e.message.contains("Binding transformer target")
                && e.message.contains("missingBinding")
        }),
        "expected unresolved-binding error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn relation_with_resolved_binding_validates_clean() {
    // `Binding` itself isn't a core Pure element kind — Java treats
    // the transformer's `binding` slot as resolving to *any* element
    // (`ImportStub.idOrPath`). We mirror that loose check, so a
    // plain Class can stand in.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        Class my::test::FakeBinding
        {
        }

        function my::test::myRelFn() : meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>[1]
        {
          ^meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>()
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Relation
          {
            ~func my::test::myRelFn():meta::pure::metamodel::relation::Relation<meta::pure::metamodel::type::Any>[1]
            legalName : Binding my::test::FakeBinding : name_col
          }
        )
    "};
    let file = parse("relation_clean_binding.pure", source);
    let errors = compile(vec![file]);
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("Binding transformer target")),
        "resolved binding must not produce a transformer error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
