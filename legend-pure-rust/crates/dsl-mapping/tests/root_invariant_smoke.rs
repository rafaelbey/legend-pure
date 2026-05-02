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

//! M2 invariant: when a class is mapped by more than one
//! set-implementation directly within a Mapping, exactly one must
//! be marked root with `*`. Mirrors Java's `TestRoot`:
//!
//!   - `testRoot` — 3 set-impls, 1 root → OK.
//!   - `testRootError` — 3 set-impls, 2 roots → error.
//!   - `testRootWithInclude` — root flow across `include` does NOT
//!     fold into the count: each Mapping is checked in isolation,
//!     so two included mappings each contributing their own root
//!     is fine.
//!
//! Single-set-impl-no-root is also fine — the implicit single
//! mapping is its own root.

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

#[test]
fn three_set_impls_one_root_validates_clean() {
    // Mirrors Java TestRoot.testRoot.
    let source = indoc! {r"
        ###Pure
        Class my::test::Person { name : String[1]; }
        Class my::test::PersonSrc { name : String[1]; }

        function my::test::a() : meta::pure::mapping::SetImplementation[*]
        {
          []
        }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Person[op] : Operation
          {
            my::test::a__SetImplementation_MANY_(rel1, rel2)
          }

          my::test::Person[rel1] : Pure
          {
            ~src my::test::PersonSrc
            name : $src.name
          }

          my::test::Person[rel2] : Pure
          {
            ~src my::test::PersonSrc
            name : $src.name
          }
        )
    "};
    let file = parse("root_clean.pure", source);
    let errors = compile(vec![file]);
    let root_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("set implementations and has") && e.message.contains("roots")
        })
        .collect();
    assert!(
        root_errors.is_empty(),
        "expected no one-root-per-class errors; got: {:#?}",
        root_errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn three_set_impls_two_roots_errors() {
    // Mirrors Java TestRoot.testRootError. Both `op` and `rel1` are
    // marked with `*` — the validator must reject.
    let source = indoc! {r"
        ###Pure
        Class my::test::Person { name : String[1]; }
        Class my::test::PersonSrc { name : String[1]; }

        function my::test::a() : meta::pure::mapping::SetImplementation[*]
        {
          []
        }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Person[op] : Operation
          {
            my::test::a__SetImplementation_MANY_(rel1, rel2)
          }

          *my::test::Person[rel1] : Pure
          {
            ~src my::test::PersonSrc
            name : $src.name
          }

          my::test::Person[rel2] : Pure
          {
            ~src my::test::PersonSrc
            name : $src.name
          }
        )
    "};
    let file = parse("root_two.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors.iter().any(|e| {
            // Match the Java diagnostic shape: class FQN, count "3
            // set implementations", root count "2 roots".
            e.message.contains("'my::test::Person'")
                && e.message.contains("3 set implementations")
                && e.message.contains("2 roots")
        }),
        "expected one-root-per-class error mentioning 3 set impls and 2 roots; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn three_set_impls_zero_roots_errors() {
    // Java's TestRoot doesn't have an explicit "0 roots" test, but
    // the rule is "exactly one root", so 0 is also a violation. The
    // diagnostic must distinguish 0-roots from N-roots so users get
    // a clear "you forgot the *" hint.
    let source = indoc! {r"
        ###Pure
        Class my::test::Person { name : String[1]; }
        Class my::test::PersonSrc { name : String[1]; }

        function my::test::a() : meta::pure::mapping::SetImplementation[*]
        {
          []
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Person[op] : Operation
          {
            my::test::a__SetImplementation_MANY_(rel1, rel2)
          }

          my::test::Person[rel1] : Pure
          {
            ~src my::test::PersonSrc
            name : $src.name
          }

          my::test::Person[rel2] : Pure
          {
            ~src my::test::PersonSrc
            name : $src.name
          }
        )
    "};
    let file = parse("root_zero.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors.iter().any(|e| {
            e.message.contains("'my::test::Person'")
                && e.message.contains("3 set implementations")
                && e.message.contains("0 roots")
        }),
        "expected one-root-per-class error mentioning 3 set impls and 0 roots; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn single_set_impl_without_explicit_root_validates_clean() {
    // The rule only fires when count > 1 — a single set-impl is
    // implicitly its own root regardless of the `*` marker. Mirrors
    // Java behavior: TestRoot.testRoot's single-Person mappings in
    // myMap1 / myMap2 each have just two set-impls with one root
    // marked, but a single-set-impl-no-root case (used widely
    // across other test files) compiles fine.
    let source = indoc! {r"
        ###Pure
        Class my::test::Person { name : String[1]; }
        Class my::test::PersonSrc { name : String[1]; }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Person : Pure
          {
            ~src my::test::PersonSrc
            name : $src.name
          }
        )
    "};
    let file = parse("root_one.pure", source);
    let errors = compile(vec![file]);
    let root_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("set implementations and has") && e.message.contains("roots")
        })
        .collect();
    assert!(
        root_errors.is_empty(),
        "single set-impl without explicit root must validate clean; got: {:#?}",
        root_errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn root_check_is_per_mapping_not_transitive_across_includes() {
    // Mirrors Java TestRoot.testRootWithInclude. myMap1 and myMap2
    // each have one root on Person; includeMap pulls both in via
    // `include`. The "exactly one root" rule must NOT fold across
    // includes — each Mapping is checked in isolation, so the
    // includer (which directly defines zero set-impls of Person)
    // sees zero roots and that's fine.
    let source = indoc! {r"
        ###Pure
        Class my::test::Person { name : String[1]; }
        Class my::test::PersonSrc { name : String[1]; }

        function my::test::a() : meta::pure::mapping::SetImplementation[*]
        {
          []
        }

        ###Mapping
        Mapping my::test::myMap1
        (
          *my::test::Person[one] : Operation
          {
            my::test::a__SetImplementation_MANY_()
          }

          my::test::Person[two] : Operation
          {
            my::test::a__SetImplementation_MANY_()
          }
        )

        Mapping my::test::myMap2
        (
          *my::test::Person[one_1] : Operation
          {
            my::test::a__SetImplementation_MANY_()
          }

          my::test::Person[two_1] : Operation
          {
            my::test::a__SetImplementation_MANY_()
          }
        )

        Mapping my::test::includeMap
        (
          include my::test::myMap1
          include my::test::myMap2
        )
    "};
    let file = parse("root_include.pure", source);
    let errors = compile(vec![file]);
    let root_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("set implementations and has") && e.message.contains("roots")
        })
        .collect();
    assert!(
        root_errors.is_empty(),
        "root check must be per-mapping, not transitive across includes; got: {:#?}",
        root_errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn xstore_bodies_do_not_count_toward_root_check() {
    // XStore bodies target Associations, not Classes. The
    // root-per-class rule must skip them — otherwise a Mapping
    // with multiple set-impls of one Class plus an unrelated
    // XStore would incorrectly fold the XStore's Association FQN
    // into the per-class count.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm { id : String[1]; }
        Class my::test::Person { firmId : String[1]; }

        Association my::test::Firm_Person
        {
          firm : my::test::Firm[1];
          employees : my::test::Person[*];
        }

        Class my::test::FirmSrc { id : String[1]; }
        Class my::test::PersonSrc { firmId : String[1]; }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Firm[firm_set] : Pure { ~src my::test::FirmSrc id : $src.id }
          *my::test::Person[employee_set] : Pure { ~src my::test::PersonSrc firmId : $src.firmId }

          my::test::Firm_Person : XStore
          {
            firm[employee_set, firm_set]      : $this.firmId == $that.id,
            employees[firm_set, employee_set] : $this.id == $that.firmId
          }
        )
    "};
    let file = parse("root_xstore.pure", source);
    let errors = compile(vec![file]);
    let root_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("set implementations and has") && e.message.contains("roots")
        })
        .collect();
    assert!(
        root_errors.is_empty(),
        "XStore bodies must not contribute to the root-per-class count; got: {:#?}",
        root_errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
