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

//! Compilation error types for the Pure compiler pipeline.

use legend_pure_parser_ast::SourceInfo;
use serde::Serialize;
use smol_str::SmolStr;

/// Diagnostic severity, used by IDE/LSP integrations.
///
/// Today every [`CompilationError`] resolves to [`Severity::Error`] — there are
/// no warning-tier diagnostics yet — but the enum + [`CompilationError::severity`]
/// derivation are wired up so warning-emitting passes can land without churning
/// the ~100 existing error construction sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Halts compilation; the model is partial.
    Error,
    /// Suspicious but not fatal; e.g. unused import, deprecated stereotype.
    Warning,
    /// Informational hint; e.g. style or migration suggestion.
    Info,
    /// Editor hint with no diagnostic weight.
    Hint,
}

/// A compilation error produced during AST → Pure lowering or validation.
#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize)]
#[error("{source_info}: {message}")]
pub struct CompilationError {
    /// Human-readable error message.
    pub message: String,
    /// Source location where the error occurred.
    pub source_info: SourceInfo,
    /// Error kind for programmatic handling.
    pub kind: CompilationErrorKind,
}

impl CompilationError {
    /// Diagnostic severity, derived from [`CompilationErrorKind`].
    ///
    /// Returns [`Severity::Error`] for every kind today. When warning-tier
    /// passes are added (e.g. unused-import lints), match on `kind` here to
    /// promote them — no call-site changes required.
    #[must_use]
    pub fn severity(&self) -> Severity {
        // No warning-tier kinds yet — every variant resolves to Error.
        let _ = &self.kind;
        Severity::Error
    }
}

/// Classification of compilation errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "tag", rename_all = "camelCase")]
pub enum CompilationErrorKind {
    /// An element path could not be resolved.
    UnresolvedElement {
        /// The unresolved path as written.
        path: SmolStr,
    },
    /// A duplicate element name in the same package.
    DuplicateElement {
        /// The duplicate name.
        name: SmolStr,
    },
    /// A variable hides or shadows an existing variable in scope.
    DuplicateVariable {
        /// The duplicate variable name.
        name: SmolStr,
    },
    /// A cyclic inheritance chain was detected.
    CyclicInheritance {
        /// The element that starts the cycle.
        element_name: SmolStr,
    },
    /// An association has invalid structure (e.g., wrong property count).
    InvalidAssociation {
        /// The association name.
        name: SmolStr,
        /// Human-readable reason.
        reason: SmolStr,
    },
    /// A class's supertype is not a Class element.
    InvalidSuperType {
        /// The class name.
        class_name: SmolStr,
        /// The invalid supertype name.
        super_name: SmolStr,
    },
    /// An annotation (stereotype or tagged value) is invalid.
    InvalidAnnotation {
        /// The element carrying the annotation.
        element_name: SmolStr,
        /// Human-readable reason.
        reason: SmolStr,
    },
    /// Two properties in the same class have the same name.
    DuplicateProperty {
        /// The class name.
        class_name: SmolStr,
        /// The duplicate property name.
        property_name: SmolStr,
    },
    /// An unqualified name resolves to multiple elements via imports.
    AmbiguousImport {
        /// The ambiguous simple name.
        name: SmolStr,
        /// Fully qualified paths of the matching candidates.
        candidates: Vec<SmolStr>,
    },
    /// An expression variant is not yet implemented in the lowering phase.
    UnsupportedExpression {
        /// The expression kind name.
        kind: SmolStr,
    },
    /// A source file failed to fully parse — some elements were recovered.
    ParseFailure {
        /// The source file that failed.
        source: SmolStr,
    },
    /// A property does not exist on the receiver type (or any supertype).
    UnknownProperty {
        /// Name of the receiver type that was searched.
        type_name: SmolStr,
        /// The unknown property name.
        property_name: SmolStr,
    },
    /// A qualified property was called with the wrong number of arguments.
    QualifiedPropertyArityMismatch {
        /// Name of the receiver type.
        type_name: SmolStr,
        /// The qualified property name.
        property_name: SmolStr,
        /// Number of parameters declared.
        expected: usize,
        /// Number of arguments supplied.
        actual: usize,
    },
    /// A qualified property argument has a type incompatible with the
    /// declared parameter (wrong type or wrong multiplicity).
    QualifiedPropertyArgTypeMismatch {
        /// Name of the receiver type.
        type_name: SmolStr,
        /// The qualified property name.
        property_name: SmolStr,
        /// Zero-based index of the offending argument.
        param_index: usize,
        /// Parameter name (for diagnostics).
        param_name: SmolStr,
        /// Expected param type (rendered).
        expected: SmolStr,
        /// Inferred argument type (rendered).
        actual: SmolStr,
    },
    /// A lambda has parameters with no type annotation and no caller-side
    /// expectation to bind them — they would silently default to `Any[1]`,
    /// which then makes operator dispatch in the body ambiguous. Emitted
    /// eagerly at the lambda so the user sees the real cause instead of a
    /// cascading "Ambiguous function call" downstream.
    CannotInferLambdaParameterTypes {
        /// Names of the parameters that could not be inferred.
        names: Vec<SmolStr>,
    },
    /// A generic type parameter declared in a function's signature
    /// (e.g. `T` in `eval<T>(...)`) was not bound from any call-site
    /// argument. After dispatch + binding, the substituted return
    /// type still contained `Generic(T)`. Java-parity:
    /// `TypeInference.java:87-89` ("The type parameter X was not
    /// resolved").
    UnresolvedTypeParameter {
        /// Name of the function whose call site triggered the error.
        function: SmolStr,
        /// Name of the unresolved generic type parameter.
        parameter: SmolStr,
    },
    /// A generic multiplicity parameter declared in a function's
    /// signature (e.g. `m` in `f<T|m>(...)`) was not bound from any
    /// call-site argument. After dispatch + binding, the substituted
    /// return multiplicity still contained `Variable(m)`. Java-parity:
    /// `TypeInference.java:102` ("The multiplicity parameter X was
    /// not resolved").
    UnresolvedMultiplicityParameter {
        /// Name of the function whose call site triggered the error.
        function: SmolStr,
        /// Name of the unresolved generic multiplicity parameter.
        parameter: SmolStr,
    },
    /// A reference targets a packageable element whose home repo is not
    /// in the use-site repo's declared dependencies. Java-parity:
    /// `VisibilityValidation.throwRepoVisibilityException`.
    NotVisible {
        /// Fully-qualified path of the target element (e.g.
        /// `"datamarts::datamt::domain::TestClass2"`).
        target_fqn: SmolStr,
        /// Source path of the use site (e.g. `"/system/testFile.pure"`).
        source_id: SmolStr,
    },
    /// A reference targets a `<<access.private>>` or `<<access.protected>>`
    /// element from a package that the access rule disallows. Java-parity:
    /// `VisibilityValidation.throwAccessException` /
    /// `Visibility.isVisibleInPackage`.
    NotAccessible {
        /// Java-shape descriptor of the target. Functions render as
        /// `pkg::name(Type[mult], …):Return[mult]`; classes/associations
        /// render as their `::`-joined FQN.
        target_fqn: SmolStr,
        /// `::`-joined FQN of the use-site package (the package of the
        /// containing top-level element). Empty string for the root.
        use_site_package: SmolStr,
    },
    /// An element carries more than one stereotype on the
    /// `meta::pure::profiles::access` profile. Java-parity:
    /// `AccessLevelValidator` "has multiple access level stereotypes".
    MultipleAccessLevels {
        /// Java-shape descriptor of the offending element.
        element_fqn: SmolStr,
    },
    /// An access stereotype was applied to something other than a class
    /// or function (most commonly a property). Java-parity:
    /// `AccessLevelValidator` "Only classes and functions may have an
    /// access level".
    AccessLevelNotAllowed {
        /// FQN of the offending element (usually `Class::propertyName`).
        element_fqn: SmolStr,
        /// Reason text (the user-facing rule).
        reason: SmolStr,
    },
}
