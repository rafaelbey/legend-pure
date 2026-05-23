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

//! Diagnostic conversion: Pure [`CompilationError`] → LSP [`Diagnostic`].

use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use tower_lsp_server::ls_types::{Diagnostic, NumberOrString};

use crate::convert;

/// Lightweight, JSON-friendly view of a [`CompilationError`] that the
/// `legend.applyEdit` execute-command embeds in its
/// [`crate::handlers::ApplyEditResult`] response. Keeping the wire
/// shape independent of the LSP [`Diagnostic`] type lets agents
/// consume the result over the executeCommand return value without
/// pulling in the LSP types package.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiagnosticRow {
    /// Canonical source path the diagnostic refers to.
    pub source: String,
    /// 1-based start line of the diagnostic span.
    pub line: u32,
    /// 1-based start column of the diagnostic span.
    pub column: u32,
    /// Severity (`error` / `warning`).
    pub severity: String,
    /// Diagnostic message.
    pub message: String,
}

impl DiagnosticRow {
    /// Build a [`DiagnosticRow`] from a [`CompilationError`].
    #[must_use]
    pub fn from_error(err: &CompilationError) -> Self {
        Self {
            source: err.source_info.source.to_string(),
            line: err.source_info.start_line,
            column: err.source_info.start_column,
            severity: "error".to_string(),
            message: err.message.clone(),
        }
    }
}

/// Static source identifier published with every diagnostic — surfaces
/// in IDEs as the "source" tag (e.g. `legend-pure (E0001)`).
pub const DIAGNOSTIC_SOURCE: &str = "legend-pure";

/// Convert a single [`CompilationError`] to an LSP [`Diagnostic`].
#[must_use]
pub fn to_diagnostic(err: &CompilationError) -> Diagnostic {
    Diagnostic {
        range: convert::range_from_source_info(&err.source_info),
        severity: Some(convert::diagnostic_severity(err.severity())),
        code: Some(NumberOrString::String(error_code(&err.kind).to_string())),
        code_description: None,
        source: Some(DIAGNOSTIC_SOURCE.to_string()),
        message: err.message.clone(),
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Stable error code per [`CompilationErrorKind`] variant. These match
/// the variant tag used in the JSON serialization (camelCase) so
/// editor pages and CI tools can cross-reference.
#[must_use]
pub fn error_code(kind: &CompilationErrorKind) -> &'static str {
    match kind {
        CompilationErrorKind::UnresolvedElement { .. } => "unresolvedElement",
        CompilationErrorKind::DuplicateElement { .. } => "duplicateElement",
        CompilationErrorKind::DuplicateVariable { .. } => "duplicateVariable",
        CompilationErrorKind::CyclicInheritance { .. } => "cyclicInheritance",
        CompilationErrorKind::InvalidAssociation { .. } => "invalidAssociation",
        CompilationErrorKind::InvalidSuperType { .. } => "invalidSuperType",
        CompilationErrorKind::InvalidAnnotation { .. } => "invalidAnnotation",
        CompilationErrorKind::DuplicateProperty { .. } => "duplicateProperty",
        CompilationErrorKind::PropertyConflict { .. } => "propertyConflict",
        CompilationErrorKind::AmbiguousImport { .. } => "ambiguousImport",
        CompilationErrorKind::UnsupportedExpression { .. } => "unsupportedExpression",
        CompilationErrorKind::ParseFailure { .. } => "parseFailure",
        CompilationErrorKind::UnknownProperty { .. } => "unknownProperty",
        CompilationErrorKind::QualifiedPropertyArityMismatch { .. } => {
            "qualifiedPropertyArityMismatch"
        }
        CompilationErrorKind::QualifiedPropertyArgTypeMismatch { .. } => {
            "qualifiedPropertyArgTypeMismatch"
        }
        CompilationErrorKind::CannotInferLambdaParameterTypes { .. } => {
            "cannotInferLambdaParameterTypes"
        }
        CompilationErrorKind::NotVisible { .. } => "notVisible",
        CompilationErrorKind::NotAccessible { .. } => "notAccessible",
        CompilationErrorKind::MultipleAccessLevels { .. } => "multipleAccessLevels",
        CompilationErrorKind::AccessLevelNotAllowed { .. } => "accessLevelNotAllowed",
        CompilationErrorKind::UnresolvedTypeParameter { .. } => "unresolvedTypeParameter",
        CompilationErrorKind::UnresolvedMultiplicityParameter { .. } => {
            "unresolvedMultiplicityParameter"
        }
        CompilationErrorKind::PackageNotInRepoPattern { .. } => "packageNotInRepoPattern",
        CompilationErrorKind::PropertyDefaultValueIncompatible { .. } => {
            "propertyDefaultValueIncompatible"
        }
        CompilationErrorKind::ConstructorMissingRequiredProperty { .. } => {
            "constructorMissingRequiredProperty"
        }
        CompilationErrorKind::ConstructorPropertyTypeMismatch { .. } => {
            "constructorPropertyTypeMismatch"
        }
        CompilationErrorKind::UndeclaredMultiplicityParameter { .. } => {
            "undeclaredMultiplicityParameter"
        }
        CompilationErrorKind::TypeMismatch { .. } => "typeMismatch",
        CompilationErrorKind::MultiplicityMismatch { .. } => "multiplicityMismatch",
        CompilationErrorKind::ConstraintBodyTypeMismatch { .. } => "constraintBodyTypeMismatch",
        CompilationErrorKind::MilestoningStereotypeConflict { .. } => {
            "milestoningStereotypeConflict"
        }
        CompilationErrorKind::MilestoningReservedPropertyName { .. } => {
            "milestoningReservedPropertyName"
        }
        CompilationErrorKind::MilestoningHierarchyMismatch { .. } => "milestoningHierarchyMismatch",
        CompilationErrorKind::MilestoningEdgePointCollision { .. } => {
            "milestoningEdgePointCollision"
        }
        CompilationErrorKind::MilestoningMissingDateContext { .. } => {
            "milestoningMissingDateContext"
        }
        CompilationErrorKind::MilestoningLatestOutsideMilestoningContext { .. } => {
            "milestoningLatestOutsideMilestoningContext"
        }
        CompilationErrorKind::MilestoningLatestNotAllowedInRange { .. } => {
            "milestoningLatestNotAllowedInRange"
        }
    }
}
