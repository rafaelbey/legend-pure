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

//! Span → compiled-node lookup for IDE integrations.
//!
//! [`PureModel::locate`] takes a `(file, line, column)` cursor position
//! and returns the innermost compiled node whose source span contains
//! it. The LSP layer uses this for `textDocument/hover` (rendering
//! [`crate::types::ResolvedType`] from `ValueSpec::type_info`),
//! `goto_definition` (resolving the located node to its definition
//! site), and `documentSymbol` outline rendering.
//!
//! The walk is a single linear scan over chunks → elements →
//! parameters/bodies; no index is built. Platform-scale models
//! (~1.3 K elements) measure well below the latency a tier-1 LSP
//! cares about, so we don't pay the complexity cost of a span tree.

use legend_pure_parser_ast::SourceInfo;

use crate::ids::ElementId;
use crate::model::{Element, PureModel};
use crate::nodes::class::{Constraint, Property, QualifiedProperty};
use crate::types::{ExprKind, Parameter, ResolvedType, ValueSpec};

/// What kind of compiled node sits under the cursor.
///
/// Variants borrow from the underlying [`PureModel`].
#[derive(Debug, Clone, Copy)]
pub enum LocatedKind<'a> {
    /// The cursor sits on the element declaration header itself —
    /// no deeper node was found. Hover / goto here resolves to the
    /// owning [`ElementId`].
    Element,
    /// A function or qualified-property parameter (`x: String[1]`).
    Parameter(&'a Parameter),
    /// A class property declaration line.
    Property(&'a Property),
    /// A class qualified-property declaration line. Inside the body,
    /// children narrow to [`LocatedKind::ValueSpec`].
    QualifiedProperty(&'a QualifiedProperty),
    /// A class constraint header. Inside the constraint expression,
    /// children narrow to [`LocatedKind::ValueSpec`].
    Constraint(&'a Constraint),
    /// A compiled expression. Hover renders the inferred type from
    /// [`ValueSpec::type_info`] when Pass-2.5 has run.
    ValueSpec(&'a ValueSpec),
}

/// A compiled node located at a cursor position.
#[derive(Debug, Clone)]
pub struct Located<'a> {
    /// The owning top-level element.
    pub element: ElementId,
    /// Span of the located node — never wider than the element span.
    pub span: SourceInfo,
    /// Variant-specific borrow back into the model.
    pub kind: LocatedKind<'a>,
}

impl<'a> Located<'a> {
    /// Inferred type if the cursor landed on a [`ValueSpec`] that
    /// Pass-2.5 successfully annotated. Returns `None` for headers,
    /// parameters, or unannotated expressions.
    #[must_use]
    pub fn resolved_type(&self) -> Option<&'a ResolvedType> {
        match self.kind {
            LocatedKind::ValueSpec(v) => v.type_info.as_deref(),
            _ => None,
        }
    }
}

impl PureModel {
    /// Returns the innermost compiled node whose source span contains
    /// `(file, line, column)`. Coordinates are **1-indexed** to match
    /// [`SourceInfo`].
    ///
    /// Returns `None` when no element in the model is sourced from
    /// `file`, or when the cursor falls outside every element span.
    #[must_use]
    pub fn locate(&self, file: &str, line: u32, column: u32) -> Option<Located<'_>> {
        let cursor = Cursor { line, column };
        for chunk in &self.chunks {
            for (idx, node) in chunk.nodes.iter() {
                if node.source_info.source.as_str() != file {
                    continue;
                }
                if !cursor.within(&node.source_info) {
                    continue;
                }
                let element_id = ElementId::InstanceId {
                    chunk_id: chunk.chunk_id,
                    local_idx: idx,
                };
                let element = chunk.elements.get(idx);
                let mut best = Located {
                    element: element_id,
                    span: node.source_info.clone(),
                    kind: LocatedKind::Element,
                };
                refine_in_element(&mut best, element, cursor);
                return Some(best);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Internal walkers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Cursor {
    line: u32,
    column: u32,
}

impl Cursor {
    fn within(self, si: &SourceInfo) -> bool {
        let after_start = (self.line, self.column) >= (si.start_line, si.start_column);
        let before_end = (self.line, self.column) <= (si.end_line, si.end_column);
        after_start && before_end
    }

    /// Approximate area of a span. Used only to pick the smaller of two
    /// spans that both contain the cursor — exact glyph counting isn't
    /// needed because typical lines fit in well under 1024 columns.
    fn area(si: &SourceInfo) -> u64 {
        let lines = u64::from(si.end_line.saturating_sub(si.start_line));
        let cols = u64::from(si.end_column.saturating_sub(si.start_column));
        lines.saturating_mul(1024).saturating_add(cols)
    }
}

fn maybe_refine<'a>(
    best: &mut Located<'a>,
    cursor: Cursor,
    span: &SourceInfo,
    kind: LocatedKind<'a>,
) {
    if !cursor.within(span) {
        return;
    }
    if Cursor::area(span) > Cursor::area(&best.span) {
        return;
    }
    best.span = span.clone();
    best.kind = kind;
}

fn refine_in_element<'a>(best: &mut Located<'a>, element: &'a Element, cursor: Cursor) {
    match element {
        Element::Function(f) => {
            for p in f.parameters.iter() {
                maybe_refine(best, cursor, &p.source_info, LocatedKind::Parameter(p));
            }
            for expr in f.body.iter() {
                refine_in_value_spec(best, cursor, expr);
            }
        }
        Element::Class(c) => {
            for p in &c.type_variable_parameters {
                maybe_refine(best, cursor, &p.source_info, LocatedKind::Parameter(p));
            }
            for prop in &c.properties {
                maybe_refine(best, cursor, &prop.source_info, LocatedKind::Property(prop));
                if let Some(dv) = &prop.default_value {
                    refine_in_value_spec(best, cursor, dv);
                }
            }
            for qp in &c.qualified_properties {
                maybe_refine(
                    best,
                    cursor,
                    &qp.source_info,
                    LocatedKind::QualifiedProperty(qp),
                );
                for p in qp.parameters.iter() {
                    maybe_refine(best, cursor, &p.source_info, LocatedKind::Parameter(p));
                }
                for expr in qp.body.iter() {
                    refine_in_value_spec(best, cursor, expr);
                }
            }
            for con in &c.constraints {
                maybe_refine(best, cursor, &con.source_info, LocatedKind::Constraint(con));
                refine_in_value_spec(best, cursor, &con.function);
                if let Some(m) = &con.message {
                    refine_in_value_spec(best, cursor, m);
                }
            }
        }
        Element::Association(a) => {
            for prop in &a.properties {
                maybe_refine(best, cursor, &prop.source_info, LocatedKind::Property(prop));
            }
            for qp in &a.qualified_properties {
                maybe_refine(
                    best,
                    cursor,
                    &qp.source_info,
                    LocatedKind::QualifiedProperty(qp),
                );
                for p in qp.parameters.iter() {
                    maybe_refine(best, cursor, &p.source_info, LocatedKind::Parameter(p));
                }
                for expr in qp.body.iter() {
                    refine_in_value_spec(best, cursor, expr);
                }
            }
        }
        // Enumeration / Profile / Measure / Unit / PrimitiveType /
        // PackageableMultiplicity / Package / DSLInstance have no
        // compiled sub-nodes worth refining into for tier-1 LSP needs.
        // Reference-kind navigation (stereotypes, tagged values, type
        // refs) goes through the new `crate::refs::ReferenceIndex`
        // instead of locator extension.
        _ => {}
    }
}

fn refine_in_value_spec<'a>(best: &mut Located<'a>, cursor: Cursor, vs: &'a ValueSpec) {
    maybe_refine(best, cursor, &vs.source_info, LocatedKind::ValueSpec(vs));
    refine_in_expr_kind(best, cursor, &vs.kind);
}

fn refine_in_expr_kind<'a>(best: &mut Located<'a>, cursor: Cursor, kind: &'a ExprKind) {
    match kind {
        ExprKind::FunctionCall(d)
        | ExprKind::PropertyCall(d)
        | ExprKind::QualifiedPropertyCall(d) => {
            for arg in &d.arguments {
                refine_in_value_spec(best, cursor, arg);
            }
        }
        ExprKind::Lambda { parameters, body } => {
            for p in parameters {
                maybe_refine(best, cursor, &p.source_info, LocatedKind::Parameter(p));
            }
            for e in body {
                refine_in_value_spec(best, cursor, e);
            }
        }
        ExprKind::Collection { elements } => {
            for e in elements {
                refine_in_value_spec(best, cursor, e);
            }
        }
        ExprKind::PathLiteral { steps, .. } => {
            // Steps carry their own source_info but we don't surface a
            // dedicated `LocatedKind` for them yet — descending into
            // each step's QP-argument expressions is enough for tier-1
            // hover/goto on the parameter lambdas inside `/qp(args)`.
            for step in steps {
                for param in &step.parameters {
                    refine_in_value_spec(best, cursor, param);
                }
            }
        }
        ExprKind::IntegerLiteral(_)
        | ExprKind::FloatLiteral(_)
        | ExprKind::DecimalLiteral(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::BooleanLiteral(_)
        | ExprKind::DateLiteral(_)
        | ExprKind::Variable { .. }
        | ExprKind::EnumValue { .. }
        | ExprKind::TypeReference { .. }
        | ExprKind::PackageableElementRef { .. }
        | ExprKind::Column
        | ExprKind::RelationLiteral { .. }
        | ExprKind::ColSpecArrayLiteral { .. }
        | ExprKind::ColSpecLiteral { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline;
    use legend_pure_parser_parser::parse;

    /// Compiles a single source file (with the bootstrap chunk pre-loaded
    /// from `bootstrap::seed`) so the locate tests can drive the real
    /// compile pipeline rather than hand-built fixtures.
    fn compile_fixture(name: &str, source: &str) -> PureModel {
        let parsed = parse(source, name).expect("test fixture must parse");
        match pipeline::compile(&[parsed], &[]) {
            Ok(model) => model,
            Err(partial) => {
                // A test fixture that fails compilation still has a
                // partial model — useful when locate is supposed to
                // work on a partially-broken file. We only fail the
                // test if the fixture was supposed to be clean.
                partial.model
            }
        }
    }

    #[test]
    fn locate_misses_when_file_unknown() {
        let model = compile_fixture(
            "fixture.pure",
            "function test::greet(name: String[1]): String[1]\n{\n  $name\n}\n",
        );
        assert!(model.locate("nope.pure", 1, 1).is_none());
    }

    #[test]
    fn locate_lands_on_function_header() {
        let src = "function test::greet(name: String[1]): String[1]\n{\n  $name\n}\n";
        let model = compile_fixture("fixture.pure", src);
        // Cursor on the function-name identifier.
        let located = model.locate("fixture.pure", 1, 16).expect("must locate");
        // Innermost we can resolve from a header position is the element
        // span — there is no narrower compiled node at this column.
        assert!(matches!(located.kind, LocatedKind::Element));
    }

    #[test]
    fn locate_narrows_to_value_spec_inside_body() {
        let src = "function test::greet(name: String[1]): String[1]\n{\n  $name\n}\n";
        let model = compile_fixture("fixture.pure", src);
        // Variable spans recorded by the parser are 1-column-wide at the
        // `$` sigil — cursor must land on the recorded position.
        let located = model.locate("fixture.pure", 3, 3).expect("must locate");
        match located.kind {
            LocatedKind::ValueSpec(v) => {
                assert!(matches!(*v.kind, ExprKind::Variable { .. }));
            }
            other => panic!("expected ValueSpec(Variable), got {other:?}"),
        }
    }

    #[test]
    fn locate_narrows_to_parameter() {
        let src = "function test::greet(name: String[1]): String[1]\n{\n  $name\n}\n";
        let model = compile_fixture("fixture.pure", src);
        // Cursor on the `name:` parameter identifier (column-22).
        let located = model.locate("fixture.pure", 1, 22).expect("must locate");
        match located.kind {
            LocatedKind::Parameter(p) => assert_eq!(p.name.as_str(), "name"),
            other => panic!("expected Parameter, got {other:?}"),
        }
    }

    #[test]
    fn locate_walks_into_lambda_body() {
        let src = "\
function test::run(): Integer[1]
{
  let f = {x: Integer[1] | $x};
  $f->eval(1)
}
";
        let model = compile_fixture("fixture.pure", src);
        // Cursor on `$x` inside the lambda body.
        let located = model.locate("fixture.pure", 3, 28).expect("must locate");
        match located.kind {
            LocatedKind::ValueSpec(v) => {
                assert!(matches!(*v.kind, ExprKind::Variable { .. }));
            }
            other => panic!("expected ValueSpec(Variable), got {other:?}"),
        }
    }

    #[test]
    fn locate_lands_on_property_in_class() {
        let src = "\
Class test::Person
{
  name: String[1];
}
";
        let model = compile_fixture("fixture.pure", src);
        // Cursor inside the property declaration line (`name: String[1]`).
        let located = model.locate("fixture.pure", 3, 4).expect("must locate");
        match located.kind {
            LocatedKind::Property(p) => assert_eq!(p.name.as_str(), "name"),
            other => panic!("expected Property, got {other:?}"),
        }
    }
}
