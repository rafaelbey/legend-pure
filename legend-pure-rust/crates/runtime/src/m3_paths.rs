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
pub const PROPERTY: &classifier = "meta::pure::metamodel::function::property::Property";

/// `meta::pure::metamodel::function::property::QualifiedProperty` — heap
/// wrapper for a derived property reference, surfaced by
/// `.qualifiedProperties` on a Class.
pub const QUALIFIED_PROPERTY: &classifier =
    "meta::pure::metamodel::function::property::QualifiedProperty";

/// `meta::pure::metamodel::function::LambdaFunction` — M3 class for anonymous
/// lambdas; recognised by the `New` native's lambda-clone shortcut.
pub const LAMBDA_FUNCTION: &classifier = "meta::pure::metamodel::function::LambdaFunction";

/// `meta::pure::metamodel::function::Function` — M3 base class for all
/// functions; also accepted by the lambda-clone shortcut.
pub const FUNCTION: &classifier = "meta::pure::metamodel::function::Function";

/// `meta::pure::metamodel::extension::Stereotype` — heap wrapper for a
/// `<<stereotype>>` annotation.
pub const STEREOTYPE: &classifier = "meta::pure::metamodel::extension::Stereotype";

/// `meta::pure::metamodel::extension::TaggedValue` — heap wrapper for a
/// `{tag = 'value'}` annotation.
pub const TAGGED_VALUE: &classifier = "meta::pure::metamodel::extension::TaggedValue";

/// `meta::pure::metamodel::type::Class` — the M3 metatype every
/// user-defined Class is an instance of. Used by the type-info back-fill
/// in `New::execute` to identify a `Class<T>`-typed first argument
/// (resolve to its [`ElementId`] and compare against the inferred
/// `args[0].type_info`'s outer element). Identity-based, never
/// classifier-string compared.
pub const CLASS: &classifier = "meta::pure::metamodel::type::Class";

/// `meta::pure::metamodel::type::generics::GenericType` — heap wrapper for
/// a parameterised type instance returned by `genericType(...)`.
pub const GENERIC_TYPE: &classifier = "meta::pure::metamodel::type::generics::GenericType";

/// `meta::pure::metamodel::relationship::Generalization` — heap wrapper for
/// a single supertype edge returned by `.generalizations`.
pub const GENERALIZATION: &classifier = "meta::pure::metamodel::relationship::Generalization";

/// `meta::pure::functions::meta::SourceInformation` — return shape of the
/// `sourceInformation(Any[1]):SourceInformation[0..1]` native.
pub const SOURCE_INFORMATION: &classifier = "meta::pure::functions::meta::SourceInformation";

/// `meta::pure::functions::collection::List` — a `List<T>(values=…)` wrapper
/// used pervasively by `evaluate(Function, List[*])`.
pub const LIST: &classifier = "meta::pure::functions::collection::List";

/// `meta::pure::functions::collection::Pair` — `Pair<U,V>(first=…, second=…)`
/// wrapper underpinning `pair()` and Map entry iteration (`keyValues`).
pub const PAIR: &classifier = "meta::pure::functions::collection::Pair";

/// `meta::pure::functions::collection::MapStats` — single-property
/// (`getIfAbsentCounter:Integer[1]`) wrapper returned by the
/// `getMapStats` native; mirrors Java Pure's `PureMapStats` exposed
/// through `MapCoreInstance.getStats()`.
pub const MAP_STATS: &classifier = "meta::pure::functions::collection::MapStats";

/// `meta::pure::metamodel::type::GetterOverride` — heap wrapper carrying
/// the four override lambdas (`getterOverrideToOne`,
/// `getterOverrideToMany`, `propertyOverride`, `defaultOverride`) plus
/// the `hiddenPayload` slot. Bound to `instance.elementOverride` by
/// `dynamicNew`'s hook-bearing overloads so absent property reads can
/// dispatch through the appropriate lambda.
pub const GETTER_OVERRIDE: &classifier = "meta::pure::metamodel::type::GetterOverride";

/// `meta::pure::metamodel::valuespecification::VariableExpression` —
/// AST-metamodel node for a `$name` variable reference. Produced by
/// `deactivate(varRef)` so the AST can be introspected.
pub const VARIABLE_EXPRESSION: &classifier =
    "meta::pure::metamodel::valuespecification::VariableExpression";

/// `meta::pure::metamodel::type::FunctionType` — heap wrapper exposing
/// a lambda's `parameters: VariableExpression[*]` + `returnType: GenericType[1]`
/// + `returnMultiplicity: Multiplicity[1]`. Produced as
/// `genericType($lambda).typeArguments[0].rawType` so reflective walks
/// match Java Pure's m3 metamodel.
pub const FUNCTION_TYPE: &classifier = "meta::pure::metamodel::type::FunctionType";

/// `meta::pure::metamodel::multiplicity::Multiplicity` — heap wrapper for
/// a `[m..n]` multiplicity declaration. Used by FunctionType /
/// VariableExpression to expose lambda parameter and return-side
/// cardinalities through the reflective metamodel.
pub const MULTIPLICITY: &classifier = "meta::pure::metamodel::multiplicity::Multiplicity";

/// `meta::pure::metamodel::valuespecification::InstanceValue` —
/// AST-metamodel node for a literal / collection / pre-computed value.
/// Produced by `deactivate(lit)` or `deactivate([collection])`.
pub const INSTANCE_VALUE: &classifier = "meta::pure::metamodel::valuespecification::InstanceValue";

/// `meta::pure::metamodel::valuespecification::FunctionExpression` —
/// AST-metamodel base for function-call nodes.
pub const FUNCTION_EXPRESSION: &classifier =
    "meta::pure::metamodel::valuespecification::FunctionExpression";

/// `meta::pure::metamodel::valuespecification::SimpleFunctionExpression` —
/// AST-metamodel node for a resolved function call. Produced by
/// `deactivate(fnCall)`; carries `func`, `functionName`, and
/// `parametersValues`.
pub const SIMPLE_FUNCTION_EXPRESSION: &classifier =
    "meta::pure::metamodel::valuespecification::SimpleFunctionExpression";

/// `meta::pure::test::surveyor::TestResult` — heap-object shape returned
/// by the Pure-level test surveyor's per-test result builder.
pub const TEST_RESULT: &classifier = "meta::pure::test::surveyor::TestResult";

/// `meta::pure::profiles::equality` — Profile whose `Key` stereotype
/// marks class properties as structural-equality keys. A `<<equality.Key>>`
/// stereotype ref matches iff `profile` resolves to this FQN and
/// `value == "Key"`.
pub const EQUALITY_PROFILE: &classifier = "meta::pure::profiles::equality";

/// Resolve a `::`-qualified FQN to its [`ElementId`], returning `None` if
/// any segment doesn't resolve. Used alongside the constants above to
/// compare heap-object classifiers by structural identity instead of
/// textual suffix matching.
#[must_use]
pub fn resolve(model: &PureModel, fqn: &classifier) -> Option<ElementId> {
    if fqn.is_empty() {
        return None;
    }
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    model.resolve_by_path(&segments)
}
