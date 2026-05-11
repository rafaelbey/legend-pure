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

//! [`IslandLowerer`] for the TDS inline-island.
//!
//! Lowers `#TDS\n cols\n rows\n#` into the synthetic AST
//!
//! ```text
//! meta::pure::metamodel::relation::stringToTDS('<canonical-csv>')
//!     ->cast(@meta::pure::metamodel::relation::TDS<(col1:T1[m1], col2:T2[m2], ...)>)
//! ```
//!
//! The runtime native [`stringToTDS`] (in `legend-pure-runtime`)
//! consumes the CSV string and produces the same `ParsedTDS`-bearing
//! TDS instance. The cast supplies the compile-time `T` parameter
//! (a relation type with per-column types/multiplicities inferred or
//! overridden) so downstream `over`/`extend`/`sort`/etc. can dispatch
//! against typed columns. The cast is compile-time only — it doesn't
//! affect runtime behaviour.
//!
//! The shared CSV parse + per-column type inference lives in
//! [`crate::csv`] and is reused verbatim by the runtime native, so the
//! `#TDS#` form and a literal `stringToTDS('<csv>')` call produce
//! bit-for-bit identical runtime values. The only difference is that
//! `#TDS#` carries a typed `T` at compile time; `stringToTDS` returns
//! `TDS<Any>` (the compiler doesn't know the column types from an
//! opaque string literal).

use legend_pure_parser_ast::annotation::PackageableElementPtr;
use legend_pure_parser_ast::expression::{
    ArrowFunction, Expression, FunctionApplication, Literal, StringLiteral, TypeReferenceExpr,
};
use legend_pure_parser_ast::island::IslandContent;
use legend_pure_parser_ast::source_info::SourceInfo;
use legend_pure_parser_ast::type_ref::{
    Multiplicity as AstMultiplicity, MultiplicityArgument, Package, RELATION_TYPE_SENTINEL,
    TypeReference, TypeSpec,
};
use legend_pure_parser_pure::island_lower::IslandLowerer;
use smol_str::SmolStr;

use crate::ast::{TDSCell, TDSColumn, TDSExpr};
use crate::csv::{self, ColumnOverride, ColumnType, ParsedTDS};

// ---------------------------------------------------------------------------
// FQNs of the symbols the synthetic AST references
// ---------------------------------------------------------------------------

const RELATION_PACKAGE: &[&str] = &["meta", "pure", "metamodel", "relation"];
const STRING_TO_TDS_NAME: &str = "stringToTDS";
const CAST_PACKAGE: &[&str] = &["meta", "pure", "functions", "lang"];
const CAST_NAME: &str = "cast";

// ---------------------------------------------------------------------------
// IslandLowerer impl
// ---------------------------------------------------------------------------

/// Lowerer for the TDS island.
pub struct TDSIslandLowerer;

impl IslandLowerer for TDSIslandLowerer {
    fn tag(&self) -> &str {
        crate::ast::TAG
    }

    fn lower_to_ast(
        &self,
        content: &dyn IslandContent,
        source_info: &SourceInfo,
    ) -> Option<Expression> {
        let tds = content.as_any().downcast_ref::<TDSExpr>()?;

        // Build the canonical CSV: header line (column names) followed
        // by data rows. Mirrors Java's `TDSExtension.parse`:
        //
        //   givenRelationType._columns().collect(_name).makeString(", ")
        //       + "\n" + body
        let canonical_csv = reconstruct_csv(tds);

        // Build per-column overrides from explicit `name:Type[mult]`
        // declarations in the source. Anything missing → infer from data.
        let overrides = column_overrides(&tds.columns);

        // Run the shared parse + inference. Same call the runtime
        // `stringToTDS` native makes; same output.
        let parsed = csv::parse_and_infer(&canonical_csv, &overrides).ok()?;

        // Build `stringToTDS('<csv>')` function-application AST.
        let csv_literal = string_literal(&canonical_csv, source_info.clone());
        let to_tds_call = function_call(
            string_to_tds_ptr(source_info.clone()),
            vec![csv_literal],
            source_info.clone(),
        );

        // Wrap with `->cast(@TDS<(col specs)>)`.
        let cast_call = arrow_cast_to_typed_tds(to_tds_call, &parsed, source_info.clone());

        Some(cast_call)
    }
}

/// Convenience helper — `vec![Box::new(TDSIslandLowerer)]` for
/// callers that want the TDS DSL plug-in registered alongside any
/// other island lowerers.
#[must_use]
pub fn default_island_lowerers() -> Vec<Box<dyn IslandLowerer>> {
    vec![Box::new(TDSIslandLowerer)]
}

// ---------------------------------------------------------------------------
// CSV reconstruction
// ---------------------------------------------------------------------------

/// Reconstruct the CSV body from a parsed [`TDSExpr`]. Header line is
/// the column names (just names — types are stripped here; the
/// runtime/compile-time inference re-derives them); each data row
/// joins its raw cell texts with `, `; rows separated by `\n`.
pub(crate) fn reconstruct_csv(tds: &TDSExpr) -> String {
    let mut buf = String::new();
    let header: Vec<&str> = tds.columns.iter().map(|c| c.name.as_str()).collect();
    buf.push_str(&header.join(", "));
    for row in &tds.rows {
        buf.push('\n');
        let cells: Vec<&str> = row.iter().map(|c: &TDSCell| c.raw.as_str()).collect();
        buf.push_str(&cells.join(", "));
    }
    buf
}

// ---------------------------------------------------------------------------
// Column-override extraction
// ---------------------------------------------------------------------------

pub(crate) fn column_overrides(columns: &[TDSColumn]) -> Vec<ColumnOverride> {
    columns
        .iter()
        .map(|col| match &col.type_ref {
            None => ColumnOverride::default(),
            Some(t) => ColumnOverride {
                type_tag: Some(classify_type_name(t.name.as_str())),
                multiplicity: t
                    .multiplicity
                    .as_ref()
                    .and_then(|m| parse_multiplicity_str(m.as_str())),
            },
        })
        .collect()
}

/// Map a Pure-side type name in a `#TDS\n cols\n…\n#` header to our
/// [`ColumnType`] enum. The seven primitives map to their dedicated
/// variants; any other identifier (including a qualified path like
/// `meta::pure::metamodel::variant::Variant`) becomes a
/// [`ColumnType::Other`] carrying the trailing class name and the
/// `::`-joined package prefix. Unrecognised primitive-like names with
/// no package qualification still fall through to [`ColumnType::Other`]
/// so the resolver — not the TDS DSL — decides whether the symbol is
/// resolvable.
fn classify_type_name(name: &str) -> ColumnType {
    match name {
        "Integer" => ColumnType::Integer,
        "Float" => ColumnType::Float,
        "Decimal" => ColumnType::Decimal,
        "Boolean" => ColumnType::Boolean,
        "String" => ColumnType::String,
        "StrictDate" | "Date" => ColumnType::StrictDate,
        "DateTime" => ColumnType::DateTime,
        _ => {
            // Qualified path: split on the final `::` separator. The
            // suffix is the bare class name; the prefix (if any) is the
            // package path. Accepts unqualified class names too.
            let (package, bare) = match name.rsplit_once("::") {
                Some((pkg, n)) => (Some(SmolStr::new(pkg)), SmolStr::new(n)),
                None => (None, SmolStr::new(name)),
            };
            ColumnType::Other {
                package,
                name: bare,
            }
        }
    }
}

/// Parse the raw bracket-contents of a multiplicity (e.g. `"1"`,
/// `"0..1"`, `"*"`, `"1..*"`) into the AST's [`Multiplicity`]
/// variants. Used only for header-supplied overrides; whatever the
/// user wrote in `[...]` after a column type.
fn parse_multiplicity_str(s: &str) -> Option<legend_pure_parser_pure::types::Multiplicity> {
    use legend_pure_parser_pure::types::Multiplicity;
    let s = s.trim();
    match s {
        "1" => Some(Multiplicity::PureOne),
        "0..1" => Some(Multiplicity::ZeroOrOne),
        "*" | "0..*" => Some(Multiplicity::ZeroOrMany),
        "1..*" => Some(Multiplicity::OneOrMany),
        _ => {
            // `[lower..upper]` — try parsing.
            let mut parts = s.splitn(2, "..");
            let lower: u32 = parts.next()?.parse().ok()?;
            let upper_str = parts.next()?;
            if upper_str == "*" {
                Some(Multiplicity::Range { lower, upper: None })
            } else {
                let upper: u32 = upper_str.parse().ok()?;
                Some(Multiplicity::Range {
                    lower,
                    upper: Some(upper),
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AST builders
// ---------------------------------------------------------------------------

fn string_literal(value: &str, source_info: SourceInfo) -> Expression {
    Expression::Literal(Literal::String(StringLiteral {
        value: SmolStr::new(value),
        source_info,
    }))
}

/// Build a [`Package`] from `seg1::seg2::…` segments. Source info is
/// shared across all segments — the synthetic package is purely a
/// reference target, not a distinct source-form construct.
fn build_package(segments: &[&str], source_info: SourceInfo) -> Option<Package> {
    let mut iter = segments.iter();
    let first = iter.next()?;
    let mut pkg = Package::root(SmolStr::new(*first), source_info.clone());
    for seg in iter {
        pkg = pkg.child(SmolStr::new(*seg), source_info.clone());
    }
    Some(pkg)
}

/// Build a [`PackageableElementPtr`] for a fully-qualified name.
fn pkg_ptr(segments: &[&str], name: &str, source_info: SourceInfo) -> PackageableElementPtr {
    PackageableElementPtr {
        package: build_package(segments, source_info.clone()),
        name: SmolStr::new(name),
        source_info,
    }
}

fn string_to_tds_ptr(source_info: SourceInfo) -> PackageableElementPtr {
    pkg_ptr(RELATION_PACKAGE, STRING_TO_TDS_NAME, source_info)
}

fn cast_ptr(source_info: SourceInfo) -> PackageableElementPtr {
    pkg_ptr(CAST_PACKAGE, CAST_NAME, source_info)
}

fn function_call(
    function: PackageableElementPtr,
    arguments: Vec<Expression>,
    source_info: SourceInfo,
) -> Expression {
    Expression::FunctionApplication(FunctionApplication {
        function,
        arguments,
        source_info,
    })
}

/// Build the synthetic `->cast(@TDS<RelationType<(col1:T1[m1], …)>>)`
/// arrow call wrapping `stringToTDS(<csv>)`.
///
/// `TDS<X>` requires `X` to be supplied — bare `cast(@TDS)` would
/// leave `T` undefined, which is a compile error. The columns
/// inferred from the CSV become the inner relation's structure.
///
/// Encoding shape:
///
/// ```text
/// TypeReference {
///   name: "TDS", type_arguments: [
///     TypeReference {
///       name: "RelationType", type_arguments: [
///         TypeReference {
///           name: RELATION_TYPE_SENTINEL,
///           type_arguments: [
///             TypeReference { name: col_name,
///               type_arguments: [<col primitive type>],
///               multiplicity_arguments: [Concrete(col_mult)] },
///             …
///           ]
///         }
///       ]
///     }
///   ]
/// }
/// ```
///
/// The resolver decodes the inner sentinel back to
/// `TypeExpr::Relation(cols)`, yielding the canonical resolved shape
/// `Named { TDS, [Named { RelationType, [Relation(cols)] }] }`. Column
/// names + per-column multiplicities survive through to the resolved
/// `TypeExpr` so downstream consumers (overload narrower, future
/// stricter type-checking) can read the structural shape.
fn arrow_cast_to_typed_tds(
    target: Expression,
    parsed: &ParsedTDS,
    source_info: SourceInfo,
) -> Expression {
    // Each column → a `TypeReference { name=col_name, type_arguments=[col_type],
    // multiplicity_arguments=[col_mult] }` — the encoding the resolver's
    // `RELATION_TYPE_SENTINEL` decoder expects.
    let column_refs: Vec<TypeReference> = parsed
        .columns
        .iter()
        .map(|col| {
            let inner_pkg = col.type_tag.pure_type_package().map(|p| {
                let segments: Vec<&str> = p.split("::").collect();
                build_package(&segments, source_info.clone())
            });
            TypeReference {
                package: None,
                name: col.name.clone(),
                type_arguments: vec![TypeReference {
                    package: inner_pkg.flatten(),
                    name: SmolStr::new(col.type_tag.pure_type_name()),
                    type_arguments: vec![],
                    multiplicity_arguments: vec![],
                    type_variable_values: vec![],
                    source_info: source_info.clone(),
                }],
                multiplicity_arguments: vec![MultiplicityArgument::Concrete(
                    ast_multiplicity(&col.multiplicity),
                    source_info.clone(),
                )],
                type_variable_values: vec![],
                source_info: source_info.clone(),
            }
        })
        .collect();
    // The structural relation type (sentinel-encoded).
    let structural_relation_ref = TypeReference {
        package: None,
        name: SmolStr::new(RELATION_TYPE_SENTINEL),
        type_arguments: column_refs,
        multiplicity_arguments: vec![],
        type_variable_values: vec![],
        source_info: source_info.clone(),
    };
    // `RelationType<(cols)>` wrapping the structural relation.
    let relation_type_ref = TypeReference {
        package: build_package(RELATION_PACKAGE, source_info.clone()),
        name: SmolStr::new("RelationType"),
        type_arguments: vec![structural_relation_ref],
        multiplicity_arguments: vec![],
        type_variable_values: vec![],
        source_info: source_info.clone(),
    };
    let tds_ref = TypeReference {
        package: build_package(RELATION_PACKAGE, source_info.clone()),
        name: SmolStr::new("TDS"),
        type_arguments: vec![relation_type_ref],
        multiplicity_arguments: vec![],
        type_variable_values: vec![],
        source_info: source_info.clone(),
    };
    let type_arg = TypeReferenceExpr {
        type_ref: TypeSpec::Type(tds_ref),
        source_info: source_info.clone(),
    };
    Expression::ArrowFunction(ArrowFunction {
        target: Box::new(target),
        function: cast_ptr(source_info.clone()),
        arguments: vec![Expression::TypeReferenceExpr(type_arg)],
        source_info,
    })
}

/// Translate the runtime [`Multiplicity`](legend_pure_parser_pure::types::Multiplicity)
/// shape back into the AST's [`Multiplicity`](AstMultiplicity) shape so
/// it can be embedded in a [`MultiplicityArgument::Concrete`] node.
fn ast_multiplicity(m: &legend_pure_parser_pure::types::Multiplicity) -> AstMultiplicity {
    use legend_pure_parser_pure::types::Multiplicity as PureMult;
    match m {
        PureMult::PureOne => AstMultiplicity::PureOne,
        PureMult::ZeroOrOne => AstMultiplicity::ZeroOrOne,
        PureMult::ZeroOrMany => AstMultiplicity::ZeroOrMany,
        PureMult::OneOrMany => AstMultiplicity::OneOrMany,
        PureMult::Range { lower, upper } => AstMultiplicity::Range {
            lower: *lower,
            upper: *upper,
        },
        PureMult::Variable(name) => AstMultiplicity::Variable(SmolStr::new(name.as_str())),
    }
}
