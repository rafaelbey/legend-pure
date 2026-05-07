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

//! Naming policy: Pure FQN ↔ Java FQN, identifier escaping.

use legend_pure_parser_pure::ids::{ElementId, PackageId};
use legend_pure_parser_pure::model::PureModel;
use smol_str::SmolStr;

/// Returns the FQN of an element as a sequence of path segments
/// (e.g. `["meta", "pure", "Person"]`). The last segment is the element
/// name; preceding segments are package names from the root.
pub(crate) fn pure_fqn_segments(model: &PureModel, id: ElementId) -> Vec<SmolStr> {
    match id {
        ElementId::InstanceId { .. } => {
            let node = model.get_node(id);
            let mut segments = package_segments(model, node.parent_package);
            segments.push(node.name.clone());
            segments
        }
        ElementId::Package(pkg_id) => package_segments(model, pkg_id),
    }
}

/// Returns the FQN of an element as a `::`-joined string. Rebuilds from the
/// segment walk; keep in sync with [`pure_fqn_segments`].
pub(crate) fn pure_fqn_string(model: &PureModel, id: ElementId) -> String {
    let segments = pure_fqn_segments(model, id);
    let mut out = String::new();
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            out.push_str("::");
        }
        out.push_str(seg.as_str());
    }
    out
}

fn package_segments(model: &PureModel, pkg: PackageId) -> Vec<SmolStr> {
    let mut chain: Vec<SmolStr> = Vec::new();
    let mut cur = Some(pkg);
    while let Some(p) = cur {
        let node = model.get_package(p);
        if node.parent.is_none() {
            // Root package has empty name; do not include.
            break;
        }
        chain.push(node.name.clone());
        cur = node.parent;
    }
    chain.reverse();
    chain
}

/// Returns the parent (package) segments of an element FQN. For
/// `meta::pure::Person` returns `["meta", "pure"]`.
pub(crate) fn pure_package_segments_of(model: &PureModel, id: ElementId) -> Vec<SmolStr> {
    match id {
        ElementId::InstanceId { .. } => package_segments(model, model.get_node(id).parent_package),
        ElementId::Package(pkg_id) => {
            let parent = model.get_package(pkg_id).parent;
            parent
                .map(|p| package_segments(model, p))
                .unwrap_or_default()
        }
    }
}

/// Convert a Pure package segment list to a Java sub-package string
/// (segments lower-cased only where they match Java keywords or contain
/// invalid identifier chars; otherwise preserved verbatim).
pub(crate) fn java_subpackage_from_pure_segments(segments: &[SmolStr]) -> String {
    let mut out = String::new();
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            out.push('.');
        }
        out.push_str(&safe_java_identifier(seg.as_str()));
    }
    out
}

/// Convert a Pure FQN (just-the-package portion) into a Java package suffix:
/// e.g. `["meta", "pure"]` + root `"com.example.gen"` →
/// `"com.example.gen.meta.pure"`.
pub(crate) fn join_java_package(root: &str, suffix_segments: &[SmolStr]) -> String {
    if suffix_segments.is_empty() {
        return root.to_owned();
    }
    let mut out = String::with_capacity(root.len() + 1 + suffix_segments.len() * 8);
    out.push_str(root);
    out.push('.');
    out.push_str(&java_subpackage_from_pure_segments(suffix_segments));
    out
}

/// Returns a Java-safe identifier — escapes Java reserved words and
/// non-Java-identifier characters. Names beginning with a digit get an
/// underscore prefix.
pub(crate) fn safe_java_identifier(name: &str) -> String {
    if name.is_empty() {
        return "_".to_owned();
    }
    let mut buf = String::with_capacity(name.len() + 1);
    for ch in name.chars() {
        let ok = ch.is_ascii_alphanumeric() || ch == '_' || ch == '$';
        buf.push(if ok { ch } else { '_' });
    }
    if buf.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        buf.insert(0, '_');
    }
    if is_java_keyword(&buf) {
        buf.push('_');
    }
    buf
}

/// Encodes the Pure function FQN as the static-method name on the
/// generated facade class: `meta::pure::math::plus_Integer_MANY__Integer_1_`
/// → `meta_pure_math_plus_Integer_MANY__Integer_1_`. The mangled signature
/// is preserved verbatim; only `::` separators become `_`.
pub(crate) fn java_static_method_name(pure_fqn: &str) -> String {
    let mut out = String::with_capacity(pure_fqn.len());
    let mut chars = pure_fqn.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' && chars.peek() == Some(&':') {
            chars.next();
            out.push('_');
        } else if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// Returns true for any Java reserved word (also `true`/`false`/`null`).
pub(crate) fn is_java_keyword(s: &str) -> bool {
    matches!(
        s,
        "abstract"
            | "assert"
            | "boolean"
            | "break"
            | "byte"
            | "case"
            | "catch"
            | "char"
            | "class"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extends"
            | "final"
            | "finally"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "implements"
            | "import"
            | "instanceof"
            | "int"
            | "interface"
            | "long"
            | "native"
            | "new"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "return"
            | "short"
            | "static"
            | "strictfp"
            | "super"
            | "switch"
            | "synchronized"
            | "this"
            | "throw"
            | "throws"
            | "transient"
            | "try"
            | "void"
            | "volatile"
            | "while"
            | "true"
            | "false"
            | "null"
            | "yield"
            | "var"
            | "record"
            | "sealed"
            | "permits"
            | "non-sealed"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_static_method_name_preserves_mangling() {
        assert_eq!(
            java_static_method_name("meta::pure::functions::math::plus_Integer_MANY__Integer_1_"),
            "meta_pure_functions_math_plus_Integer_MANY__Integer_1_"
        );
    }

    #[test]
    fn safe_java_identifier_escapes_keywords() {
        assert_eq!(safe_java_identifier("class"), "class_");
        assert_eq!(safe_java_identifier("default"), "default_");
        assert_eq!(safe_java_identifier("firstName"), "firstName");
    }

    #[test]
    fn safe_java_identifier_handles_dollar_and_digits() {
        assert_eq!(safe_java_identifier("$foo"), "$foo");
        assert_eq!(safe_java_identifier("9bad"), "_9bad"); // leading digit → underscore prefix
    }

    #[test]
    fn join_java_package_handles_empty_suffix() {
        let segs: Vec<SmolStr> = Vec::new();
        assert_eq!(join_java_package("com.example", &segs), "com.example");
    }

    #[test]
    fn join_java_package_concatenates() {
        let segs: Vec<SmolStr> = vec!["meta".into(), "pure".into()];
        assert_eq!(
            join_java_package("com.example", &segs),
            "com.example.meta.pure"
        );
    }
}
