// Copyright 2024 Goldman Sachs
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

//! Canonical M3 metamodel fully-qualified names.
//!
//! Centralises the FQN strings the runtime uses for:
//! - `alloc_dynamic(<FQN>)` — the classifier stored on heap wrappers
//!   (`Property`, `QualifiedProperty`, `GenericType`, etc.)
//! - `resolve_by_path(&[segments])` — resolving a classifier string back
//!   to an [`ElementId`] for wrapper-kind dispatch (see
//!   `Evaluator::apply_object_callable`).
//!
//! Single source of truth keeps the allocation site and the dispatch site
//! in sync — if the canonical FQN of one of these M3 classes changes, it
//! changes here, not in every caller.
//!
//! All constants reference classes bootstrapped from `m3.pure`; resolving
//! any of them on a fully loaded [`PureModel`] must succeed.

use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::PureModel;
use smol_str::SmolStr;

type classifier = str;

/// `meta::pure::metamodel::function::property::Property` — heap wrapper for
/// a Class property reference, surfaced by `.properties` on a Class.
pub const PROPERTY: &str = "meta::pure::metamodel::function::property::Property";

/// `meta::pure::metamodel::function::property::QualifiedProperty` — heap
/// wrapper for a derived property reference, surfaced by
/// `.qualifiedProperties` on a Class.
pub const QUALIFIED_PROPERTY: &str = "meta::pure::metamodel::function::property::QualifiedProperty";

/// `meta::pure::metamodel::function::LambdaFunction` — M3 class for anonymous
/// lambdas; recognised by the `New` native's lambda-clone shortcut.
pub const LAMBDA_FUNCTION: &str = "meta::pure::metamodel::function::LambdaFunction";

/// `meta::pure::metamodel::function::Function` — M3 base class for all
/// functions; also accepted by the lambda-clone shortcut.
pub const FUNCTION: &str = "meta::pure::metamodel::function::Function";

/// `meta::pure::metamodel::extension::Stereotype` — heap wrapper for a
/// `<<stereotype>>` annotation.
pub const STEREOTYPE: &str = "meta::pure::metamodel::extension::Stereotype";

/// `meta::pure::metamodel::extension::TaggedValue` — heap wrapper for a
/// `{tag = 'value'}` annotation.
pub const TAGGED_VALUE: &str = "meta::pure::metamodel::extension::TaggedValue";

/// `meta::pure::metamodel::type::generics::GenericType` — heap wrapper for
/// a parameterised type instance returned by `genericType(...)`.
pub const GENERIC_TYPE: &str = "meta::pure::metamodel::type::generics::GenericType";

/// `meta::pure::metamodel::relationship::Generalization` — heap wrapper for
/// a single supertype edge returned by `.generalizations`.
pub const GENERALIZATION: &str = "meta::pure::metamodel::relationship::Generalization";

/// `meta::pure::functions::meta::SourceInformation` — return shape of the
/// `sourceInformation(Any[1]):SourceInformation[0..1]` native.
pub const SOURCE_INFORMATION: &str = "meta::pure::functions::meta::SourceInformation";

/// `meta::pure::functions::collection::List` — a `List<T>(values=…)` wrapper
/// used pervasively by `evaluate(Function, List[*])`.
pub const LIST: &str = "meta::pure::functions::collection::List";

/// `meta::pure::test::surveyor::TestResult` — heap-object shape returned
/// by the Pure-level test surveyor's per-test result builder.
pub const TEST_RESULT: &str = "meta::pure::test::surveyor::TestResult";

/// Resolve a `::`-qualified FQN to its [`ElementId`], returning `None` if
/// any segment doesn't resolve. Used alongside the constants above to
/// compare heap-object classifiers by structural identity instead of
/// textual suffix matching.
#[must_use]
pub fn resolve(model: &PureModel, fqn: &str) -> Option<ElementId> {
    if fqn.is_empty() {
        return None;
    }
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    model.resolve_by_path(&segments)
}
