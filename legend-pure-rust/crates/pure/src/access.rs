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

//! Access-level abstraction (`<<access.private>>`, `<<access.protected>>`,
//! …) and Java-parity descriptor rendering used by the access-level
//! validator.
//!
//! Mirrors Java's
//! [`AccessLevel`][1] enum + helpers. The values match Java exactly so a
//! `<<meta::pure::profiles::access.private>>` stereotype on either side
//! resolves to [`AccessLevel::Private`].
//!
//! [1]: ../../legend-pure-core/legend-pure-m3-core/src/main/java/org/finos/legend/pure/m3/compiler/visibility/AccessLevel.java

use smol_str::SmolStr;

use crate::annotations::StereotypeRef;
use crate::ids::ElementId;
use crate::model::{Element, PureModel};
use crate::types::{Multiplicity, TypeExpr};

/// Standard `meta::pure::profiles::access` profile FQN. Stereotypes on
/// this profile are the only ones that contribute to access-level
/// resolution.
pub(crate) const ACCESS_PROFILE_FQN: &[&str] = &["meta", "pure", "profiles", "access"];

/// Pure access levels. Mirrors Java's enum order + names verbatim — the
/// `Display` form is the lower-case stereotype name (`"private"`,
/// `"protected"`, …) which is what the Java diagnostics also use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessLevel {
    /// No access stereotype, or `<<access.public>>` — visible everywhere.
    Public,
    /// `<<access.protected>>` — visible in the declaring package and any
    /// sub-package.
    Protected,
    /// `<<access.private>>` — visible only inside the declaring package.
    Private,
    /// `<<access.externalizable>>` — externally callable; same in-Pure
    /// visibility as `Public` (deliberately so per Java's
    /// `Visibility.isVisibleInPackage`).
    Externalizable,
}

impl AccessLevel {
    /// Lower-case stereotype name (`"private"`, `"protected"`, …).
    /// Matches Java's `AccessLevel.getName()`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Protected => "protected",
            Self::Private => "private",
            Self::Externalizable => "externalizable",
        }
    }
}

/// Resolve the `meta::pure::profiles::access` profile to its `ElementId`
/// in the given model. Returns `None` when the profile is not registered
/// (e.g. unit-test fixtures that skip the platform bootstrap) — callers
/// should treat that as "no access rules apply".
#[must_use]
pub fn access_profile_id(model: &PureModel) -> Option<ElementId> {
    let path: Vec<SmolStr> = ACCESS_PROFILE_FQN
        .iter()
        .map(|s| SmolStr::new(*s))
        .collect();
    model.resolve_by_path(&path)
}

/// Stereotypes on the access profile attached to an element. Empty for
/// elements that don't carry any (the common case) and for elements that
/// don't carry stereotypes at all (`Profile`, `Measure`, …).
pub fn access_level_stereotypes(
    element: &Element,
    access_profile: ElementId,
) -> impl Iterator<Item = &StereotypeRef> {
    element_stereotypes(element)
        .iter()
        .filter(move |s| s.profile == access_profile)
}

/// Effective access level for an element. Returns
/// [`AccessLevel::Public`] for elements with no access stereotype, for
/// elements that don't even support stereotypes, and when the access
/// profile isn't registered. Mirrors Java's
/// `AccessLevel.calculateAccessLevel`: when the profile is found and a
/// matching stereotype exists, it wins; otherwise public.
///
/// When an element somehow carries both `private` and `protected`
/// stereotypes (a separate diagnostic, [`access_level_stereotypes`] will
/// return both), this picks the *first* one — same as Java, which then
/// flags the multi-stereotype condition as its own error.
#[must_use]
pub fn access_level_of(model: &PureModel, id: ElementId) -> AccessLevel {
    let Some(profile) = access_profile_id(model) else {
        return AccessLevel::Public;
    };
    let Some(element) = model.try_get_element(id) else {
        return AccessLevel::Public;
    };
    for stereo in element_stereotypes(element) {
        if stereo.profile != profile {
            continue;
        }
        if let Some(level) = parse_access_level(&stereo.value) {
            return level;
        }
    }
    AccessLevel::Public
}

/// Parse a stereotype value off the access profile into a level. Unknown
/// values map to `None`, matching Java's
/// `AccessLevel.getLevelFromAccessStereotype` defensive throw.
#[must_use]
pub fn parse_access_level(value: &str) -> Option<AccessLevel> {
    match value {
        "public" => Some(AccessLevel::Public),
        "protected" => Some(AccessLevel::Protected),
        "private" => Some(AccessLevel::Private),
        "externalizable" => Some(AccessLevel::Externalizable),
        _ => None,
    }
}

/// Stereotype slice for the four element kinds that carry stereotypes.
/// Empty slice for the rest, so callers can iterate uniformly.
fn element_stereotypes(element: &Element) -> &[StereotypeRef] {
    match element {
        Element::Function(f) => &f.stereotypes,
        Element::Class(c) => &c.stereotypes,
        Element::Association(a) => &a.stereotypes,
        Element::Enumeration(e) => &e.stereotypes,
        _ => &[],
    }
}

/// Render the Java-shape descriptor used in the `"X is not accessible in
/// Y"` diagnostic. For functions this is
/// `pkg::name(Type[mult], …):Return[mult]`; for everything else it's the
/// `::`-joined FQN (`pkg::ClassName`).
#[must_use]
pub fn render_target_descriptor(model: &PureModel, id: ElementId) -> SmolStr {
    let element = model.try_get_element(id);
    if let Some(Element::Function(func)) = element {
        let mut s = String::new();
        let pkg_path =
            crate::purem::fqn_path::package_path(model, model.get_node(id).parent_package);
        for seg in &pkg_path {
            s.push_str(seg);
            s.push_str("::");
        }
        s.push_str(&func.function_name);
        s.push('(');
        for (i, param) in func.parameters.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            render_type_expr(model, &param.type_expr, &mut s);
            s.push('[');
            s.push_str(&render_multiplicity(&param.multiplicity));
            s.push(']');
        }
        s.push_str("):");
        render_type_expr(model, &func.return_type, &mut s);
        s.push('[');
        s.push_str(&render_multiplicity(&func.return_multiplicity));
        s.push(']');
        return SmolStr::new(s);
    }
    SmolStr::new(crate::purem::fqn_path::element_fqn_path(model, id).join("::"))
}

/// Java-shape package path string (`"pkg1::sub"`) for a use-site
/// package. Empty string for the root package.
#[must_use]
pub fn render_package_fqn(model: &PureModel, pkg_id: crate::ids::PackageId) -> SmolStr {
    SmolStr::new(crate::purem::fqn_path::package_path(model, pkg_id).join("::"))
}

fn render_type_expr(model: &PureModel, type_expr: &TypeExpr, out: &mut String) {
    match type_expr {
        TypeExpr::Named {
            element,
            type_arguments,
            ..
        } => {
            out.push_str(&model.get_node(*element).name);
            if !type_arguments.is_empty() {
                out.push('<');
                for (i, arg) in type_arguments.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    render_type_expr(model, arg, out);
                }
                out.push('>');
            }
        }
        TypeExpr::Generic(name) => out.push_str(name),
        TypeExpr::FunctionType { .. } => out.push_str("<FunctionType>"),
        TypeExpr::GenericTypeOperation {
            left: a, right: b, ..
        } => {
            render_type_expr(model, a, out);
            out.push_str(" | ");
            render_type_expr(model, b, out);
        }
        TypeExpr::Relation(_) => out.push_str("<Relation>"),
        TypeExpr::Unresolved => out.push_str("<unresolved>"),
    }
}

fn render_multiplicity(m: &Multiplicity) -> String {
    match m {
        Multiplicity::PureOne => "1".to_string(),
        Multiplicity::ZeroOrOne => "0..1".to_string(),
        Multiplicity::ZeroOrMany => "*".to_string(),
        Multiplicity::OneOrMany => "1..*".to_string(),
        Multiplicity::Range { lower, upper } => match upper {
            Some(u) if u == lower => format!("{lower}"),
            Some(u) => format!("{lower}..{u}"),
            None => format!("{lower}..*"),
        },
        Multiplicity::Variable(v) => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_levels() {
        assert_eq!(parse_access_level("private"), Some(AccessLevel::Private));
        assert_eq!(
            parse_access_level("protected"),
            Some(AccessLevel::Protected)
        );
        assert_eq!(parse_access_level("public"), Some(AccessLevel::Public));
        assert_eq!(
            parse_access_level("externalizable"),
            Some(AccessLevel::Externalizable)
        );
        assert_eq!(parse_access_level("nope"), None);
    }

    #[test]
    fn level_names_match_stereotype_values() {
        assert_eq!(AccessLevel::Private.name(), "private");
        assert_eq!(AccessLevel::Protected.name(), "protected");
        assert_eq!(AccessLevel::Public.name(), "public");
        assert_eq!(AccessLevel::Externalizable.name(), "externalizable");
    }
}
