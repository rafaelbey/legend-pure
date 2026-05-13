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

//! Parse [`PureException`](crate::error::PureException)'s `Display`
//! output into the user-facing message plus its `Full Stack:`
//! frames.

use super::types::StackFrame;

/// Decompose a [`PureException`](crate::error::PureException) string
/// into the user-facing message plus its source location.
///
/// `PureException`'s `Display` (see `error.rs:276`) emits one of two
/// shapes depending on whether the exception carried a
/// `SourceInformation`. With source info:
/// ```text
/// Assert failure (resource:foo.pure line:5 column:3)
/// "actual assertion message — may span multiple lines"
/// Full Stack:
///     frame1 <- ...
/// ```
/// Without source info (common for asserts raised through the
/// `assert` native — the exception is constructed without a
/// `SourceInformation`):
/// ```text
/// Assert failure
/// "
/// expected: 9.1
/// actual:   9.0"
/// Full Stack:
///     testNumberPow_Function_1__Boolean_1_     <-     resource:/platform/.../pow.pure line:34 column:1
///     assertEq     <-
/// ```
/// Both shapes are handled:
///   * **Body** = everything between the first `"` and the last `"`
///     that appears before `Full Stack:` (or end-of-string).
///     Internal newlines become `" | "` so the notification stays
///     readable on one line.
///   * **Source** = parsed first from the parens after the kind
///     name; if missing, falls back to the first `resource:X
///     line:Y column:Z` triple found in the call stack — that's the
///     innermost frame, which is where the user wants to navigate.
pub fn parse_failure_components(msg: &str) -> (String, Vec<StackFrame>) {
    let header = msg.lines().next().unwrap_or("");
    let body = extract_quoted_body(msg);
    let mut stack = parse_full_stack(msg);
    // If the header had inline source info, treat that as a
    // synthesized innermost frame so the user can click straight
    // to the location even on exceptions with no proper call stack
    // (older or hand-rolled errors).
    if stack.is_empty()
        && let (Some(s), Some(l), Some(c)) = parse_source_info(header)
    {
        stack.push(StackFrame {
            function: header_kind(header).to_string(),
            source: s,
            line: l,
            column: c,
        });
    }
    let message = if body.is_empty() {
        header.to_string()
    } else {
        body
    };
    (message, stack)
}

/// Pull the text between the first `"` and the last `"` that appears
/// before the `Full Stack:` marker (or end-of-string). Compresses
/// internal newlines into ` | ` separators so the body fits on one
/// notification line.
fn extract_quoted_body(msg: &str) -> String {
    let first_quote = match msg.find('"') {
        Some(i) => i,
        None => return String::new(),
    };
    let stack_at = msg.find("\nFull Stack:").unwrap_or(msg.len());
    let search_region = &msg[first_quote + 1..stack_at];
    let last_quote_rel = match search_region.rfind('"') {
        Some(i) => i,
        None => search_region.len(),
    };
    let raw = &search_region[..last_quote_rel];
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Parse every `Full Stack:` frame line into a [`StackFrame`].
///
/// Each line of the form
/// `    <name>     <-     resource:<src> line:<n> column:<n>`
/// becomes one entry. Order is preserved — exactly what
/// PureException's `Display` emits, which is innermost-first (the
/// frame nearest the actual error is at index 0). Frames that don't
/// contain a complete `resource:/line:/column:` triple are skipped
/// silently.
fn parse_full_stack(msg: &str) -> Vec<StackFrame> {
    let Some(start) = msg.find("\nFull Stack:") else {
        return Vec::new();
    };
    let stack_section = &msg[start..];
    let mut out = Vec::new();
    for line in stack_section.lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Split off the function name at `<-` so we don't pick up
        // the arrow itself as part of the name.
        let (name_part, location_part) = match trimmed.split_once("<-") {
            Some((n, l)) => (n.trim().to_string(), l),
            None => continue,
        };
        let mut source: Option<String> = None;
        let mut line_num: Option<u32> = None;
        let mut column: Option<u32> = None;
        for part in location_part.split_whitespace() {
            if let Some(v) = part.strip_prefix("resource:") {
                source = Some(v.to_string());
            } else if let Some(v) = part.strip_prefix("line:") {
                line_num = v.parse().ok();
            } else if let Some(v) = part.strip_prefix("column:") {
                column = v.parse().ok();
            }
        }
        if let (Some(s), Some(l), Some(c)) = (source, line_num, column) {
            out.push(StackFrame {
                function: name_part,
                source: s,
                line: l,
                column: c,
            });
        }
    }
    out
}

/// Strip the location parens off a header so the synthesized frame
/// is named after the kind (`Assert failure`, `Execution error`, …)
/// without trailing noise.
fn header_kind(header: &str) -> &str {
    match header.find(" (") {
        Some(i) => &header[..i],
        None => header.trim(),
    }
}

/// Pull `(source, line, column)` out of the location segment of a
/// [`PureException`](crate::error::PureException) header
/// (`... (resource:X line:Y column:Z)`).
///
/// Returns `(None, None, None)` when the parens aren't present (no
/// source info was attached to the exception). Robust against
/// whitespace inside the parens and tolerates either of the integer
/// parses failing.
fn parse_source_info(header: &str) -> (Option<String>, Option<u32>, Option<u32>) {
    let Some(open) = header.rfind('(') else {
        return (None, None, None);
    };
    let Some(close_off) = header[open + 1..].rfind(')') else {
        return (None, None, None);
    };
    let inner = &header[open + 1..open + 1 + close_off];
    let mut source: Option<String> = None;
    let mut line: Option<u32> = None;
    let mut column: Option<u32> = None;
    for part in inner.split_whitespace() {
        if let Some(v) = part.strip_prefix("resource:") {
            source = Some(v.to_string());
        } else if let Some(v) = part.strip_prefix("line:") {
            line = v.parse().ok();
        } else if let Some(v) = part.strip_prefix("column:") {
            column = v.parse().ok();
        }
    }
    (source, line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_extracted_between_quotes() {
        let msg = "Assert failure\n\"expected: 9.1\nactual:   9.0\"\nFull Stack:\n";
        let (body, _) = parse_failure_components(msg);
        assert_eq!(body, "expected: 9.1 | actual:   9.0");
    }

    #[test]
    fn header_kind_synthesized_when_no_stack() {
        let msg = "Assert failure (resource:foo.pure line:5 column:3)\n\"oops\"\n";
        let (body, stack) = parse_failure_components(msg);
        assert_eq!(body, "oops");
        assert_eq!(stack.len(), 1);
        assert_eq!(stack[0].function, "Assert failure");
        assert_eq!(stack[0].source, "foo.pure");
        assert_eq!(stack[0].line, 5);
        assert_eq!(stack[0].column, 3);
    }

    #[test]
    fn full_stack_frames_parsed() {
        let msg = "Assert failure\n\"oops\"\nFull Stack:\n    testFn     <-     resource:/x/y.pure line:10 column:1\n    assertEq     <-     resource:/x/z.pure line:99 column:5\n";
        let (_, stack) = parse_failure_components(msg);
        assert_eq!(stack.len(), 2);
        assert_eq!(stack[0].function, "testFn");
        assert_eq!(stack[0].line, 10);
        assert_eq!(stack[1].function, "assertEq");
        assert_eq!(stack[1].column, 5);
    }

    #[test]
    fn header_used_as_message_when_no_quoted_body() {
        let msg = "Plain error with no quoted body";
        let (body, stack) = parse_failure_components(msg);
        assert_eq!(body, "Plain error with no quoted body");
        assert!(stack.is_empty());
    }
}
