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

//! Shared format-string parser for Pure's `format(String[1], Any[*]): String[1]`.
//!
//! Java parity: same byte-by-byte scan as
//! `legend-pure-runtime-java-engine-interpreted/.../Format.java` and
//! `legend-pure-runtime-java-engine-compiled/.../PureStringFormat.java`.
//!
//! The runtime calls this to drive its per-specifier formatting loop
//! (`crates/runtime/src/native/string.rs::Format::execute`); the
//! compile-time validator
//! (`crates/pure/src/validate.rs::validate_format_specifiers`) calls
//! it to walk specifier positions and check arg types against the
//! lowered argument list. Shipping a single parser means the two
//! consumers stay in semantic lock-step.

use smol_str::SmolStr;

/// One `%`-specifier extracted from a format string.
///
/// Width / precision / zero-padding aren't carried here — those affect
/// runtime rendering, not type-checking. The compile-time validator
/// only needs the spec letter and the source span; the runtime keeps
/// its own inline modifier parsing alongside the per-spec
/// formatting arms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatSpec {
    /// The specifier letter: one of `s`, `r`, `d`, `f`, `t`.
    /// `%%` (literal-percent escape) is NOT emitted as a `FormatSpec`
    /// because it doesn't consume an argument.
    pub spec: char,
    /// Byte offset of the `%` character within the source template.
    /// The compile-time validator uses this to attach errors to a
    /// useful source span when the format string is itself a literal.
    pub byte_offset: usize,
}

/// Parses a Pure `format(...)` template into the list of
/// argument-consuming specifiers.
///
/// `%%` produces no entry (Java parity — it's a literal-percent
/// escape, not an arg slot). Invalid specifiers and stray `%`
/// characters are silently skipped to mirror the runtime's
/// emit-the-percent-and-keep-going behavior at
/// `crates/runtime/src/native/string.rs:583-587`.
#[must_use]
pub fn parse_format_specs(template: &str) -> Vec<FormatSpec> {
    let bytes = template.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] != b'%' {
            i += 1;
            continue;
        }
        let percent = i;
        let mut j = i + 1;
        // `%%` — literal percent, consumes no arg.
        if j < bytes.len() && bytes[j] == b'%' {
            i = j + 1;
            continue;
        }
        // `%0Nd` zero-pad prefix — the leading `0` only counts as a
        // pad flag when followed by a digit; otherwise the byte is
        // treated as part of the width digits below (matches the
        // runtime's `is_ascii_digit` check at L530).
        if j < bytes.len()
            && bytes[j] == b'0'
            && j + 1 < bytes.len()
            && bytes[j + 1].is_ascii_digit()
        {
            j += 1;
        }
        // Width digits.
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        // Optional `.precision` (digits after a `.`).
        if j < bytes.len() && bytes[j] == b'.' {
            let p_start = j + 1;
            let mut p_end = p_start;
            while p_end < bytes.len() && bytes[p_end].is_ascii_digit() {
                p_end += 1;
            }
            if p_end > p_start {
                j = p_end;
            } else {
                j = p_start;
            }
        }
        if j >= bytes.len() {
            // Trailing `%` without a specifier — skip; the runtime
            // emits a literal `%` and moves on.
            i += 1;
            continue;
        }
        let spec_byte = bytes[j];
        let spec_end = if spec_byte == b't' && j + 1 < bytes.len() && bytes[j + 1] == b'{' {
            // `%t{pattern}` — skip past the closing `}` so it doesn't
            // get re-parsed as more `%`s. Matches runtime parsing at
            // L570-578.
            let pat_start = j + 2;
            if let Some(end) = template[pat_start..].find('}') {
                pat_start + end + 1
            } else {
                j + 1
            }
        } else {
            j + 1
        };
        if !matches!(spec_byte, b's' | b'r' | b'd' | b'f' | b't') {
            // Invalid specifier — skip without emitting a FormatSpec.
            // The runtime also skips (emits a literal `%`).
            i += 1;
            continue;
        }
        out.push(FormatSpec {
            spec: spec_byte as char,
            byte_offset: percent,
        });
        i = spec_end;
    }

    out
}

/// Returns the Pure-source name of the primitive type that a given
/// specifier requires, or `None` if the specifier accepts any type.
///
/// Used by the compile-time validator to render the `expected` field
/// of `FormatSpecifierTypeMismatch`. Java parity:
/// `Format.java:105-108, 156-159, 115-118`.
#[must_use]
pub fn required_primitive_for(spec: char) -> Option<SmolStr> {
    match spec {
        'd' => Some(SmolStr::new_static("Integer")),
        'f' => Some(SmolStr::new_static("Float")),
        't' => Some(SmolStr::new_static("Date")),
        _ => None, // %s / %r accept any value at runtime.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_template() {
        assert!(parse_format_specs("").is_empty());
    }

    #[test]
    fn no_specifiers() {
        assert!(parse_format_specs("hello world").is_empty());
    }

    #[test]
    fn single_s() {
        let specs = parse_format_specs("%s");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].spec, 's');
        assert_eq!(specs[0].byte_offset, 0);
    }

    #[test]
    fn mixed_specifiers() {
        let specs = parse_format_specs("%s %d %f");
        assert_eq!(
            specs.iter().map(|s| s.spec).collect::<Vec<_>>(),
            vec!['s', 'd', 'f']
        );
        assert_eq!(
            specs.iter().map(|s| s.byte_offset).collect::<Vec<_>>(),
            vec![0, 3, 6]
        );
    }

    #[test]
    fn double_percent_consumes_no_arg() {
        // `%% %d` → only one slot (`%d`); `%%` is a literal-percent escape.
        let specs = parse_format_specs("%% %d");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].spec, 'd');
        assert_eq!(specs[0].byte_offset, 3);
    }

    #[test]
    fn zero_pad_and_precision_modifiers() {
        let specs = parse_format_specs("%05d %.2f");
        assert_eq!(
            specs.iter().map(|s| s.spec).collect::<Vec<_>>(),
            vec!['d', 'f']
        );
    }

    #[test]
    fn date_pattern_does_not_eat_following_percents() {
        // `%t{yyyy}` consumes one slot; the following `%s` is its own
        // slot. The `}` in the date pattern must not confuse the scan.
        let specs = parse_format_specs("%t{yyyy} - %s");
        assert_eq!(
            specs.iter().map(|s| s.spec).collect::<Vec<_>>(),
            vec!['t', 's']
        );
    }

    #[test]
    fn invalid_specifier_silently_skipped() {
        // `%z` isn't in the spec table; the runtime emits a literal
        // `%` and keeps going. Don't emit a FormatSpec for it.
        let specs = parse_format_specs("%z %d");
        assert_eq!(specs.iter().map(|s| s.spec).collect::<Vec<_>>(), vec!['d']);
    }

    #[test]
    fn required_types() {
        assert_eq!(
            required_primitive_for('d'),
            Some(SmolStr::new_static("Integer"))
        );
        assert_eq!(
            required_primitive_for('f'),
            Some(SmolStr::new_static("Float"))
        );
        assert_eq!(
            required_primitive_for('t'),
            Some(SmolStr::new_static("Date"))
        );
        assert_eq!(required_primitive_for('s'), None);
        assert_eq!(required_primitive_for('r'), None);
    }
}
