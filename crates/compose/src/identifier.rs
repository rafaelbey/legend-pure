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

//! Identifier quoting logic for Pure grammar.
//!
//! Pure identifiers that contain non-alphanumeric characters, start with a
//! digit, or are reserved words must be quoted with single quotes.

/// Returns the identifier as-is if it's a valid unquoted identifier,
/// or wraps it in single quotes if it needs quoting.
///
/// An identifier needs quoting if:
/// - It's empty
/// - It doesn't match `[a-zA-Z_][a-zA-Z0-9_]*`
#[must_use]
pub fn maybe_quote(id: &str) -> String {
    if needs_quoting(id) {
        format!("'{id}'")
    } else {
        id.to_string()
    }
}

/// Returns `true` if the identifier needs to be quoted.
fn needs_quoting(id: &str) -> bool {
    if id.is_empty() {
        return true;
    }
    let mut chars = id.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return true,
    }
    for c in chars {
        if !c.is_ascii_alphanumeric() && c != '_' {
            return true;
        }
    }
    false
}

/// Escapes a string for use inside single-quoted Pure literals.
///
/// Handles: single quotes (`'`), backslashes (`\`), newlines, and tabs.
/// Used for string literals, tagged values, and type variable value strings.
#[must_use]
pub fn escape_pure_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\'' => result.push_str("\\'"),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\t' => result.push_str("\\t"),
            _ => result.push(c),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_identifier() {
        assert_eq!(maybe_quote("name"), "name");
        assert_eq!(maybe_quote("_private"), "_private");
        assert_eq!(maybe_quote("Class1"), "Class1");
    }

    #[test]
    fn test_needs_quoting() {
        assert_eq!(maybe_quote("30_360"), "'30_360'");
        assert_eq!(maybe_quote("with spaces"), "'with spaces'");
        assert_eq!(maybe_quote("A-Z"), "'A-Z'");
        assert_eq!(maybe_quote("O.K."), "'O.K.'");
    }
}
