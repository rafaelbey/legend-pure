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

//! Indented text writer for composing formatted Pure grammar output.
//!
//! The [`IndentWriter`] manages a string buffer with an indentation stack,
//! producing canonically formatted Pure source text with consistent 2-space
//! indentation (matching the Java `PureGrammarComposer`).

/// A simple writer that tracks indentation level and builds a string buffer.
///
/// All element composers write through this to ensure consistent formatting.
pub struct IndentWriter {
    buf: String,
    indent: usize,
    at_line_start: bool,
}

impl IndentWriter {
    /// Creates a new `IndentWriter` with no indentation.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: String::new(),
            indent: 0,
            at_line_start: true,
        }
    }

    /// Increases the indentation level by one (2 spaces).
    pub fn push_indent(&mut self) {
        self.indent += 1;
    }

    /// Decreases the indentation level by one (2 spaces).
    pub fn pop_indent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
    }

    /// Appends text to the buffer, writing indentation first if at a line start.
    pub fn write(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.at_line_start {
            for _ in 0..self.indent {
                self.buf.push_str("  ");
            }
            self.at_line_start = false;
        }
        self.buf.push_str(s);
    }

    /// Appends a newline to the buffer.
    pub fn newline(&mut self) {
        self.buf.push('\n');
        self.at_line_start = true;
    }

    /// Writes text followed by a newline.
    pub fn write_line(&mut self, s: &str) {
        self.write(s);
        self.newline();
    }

    /// Consumes the writer and returns the built string.
    #[must_use]
    pub fn finish(self) -> String {
        self.buf
    }
}

impl Default for IndentWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_indentation() {
        let mut w = IndentWriter::new();
        w.write_line("Class A");
        w.write_line("{");
        w.push_indent();
        w.write_line("name: String[1];");
        w.pop_indent();
        w.write_line("}");

        assert_eq!(w.finish(), "Class A\n{\n  name: String[1];\n}\n");
    }

    #[test]
    fn test_write_without_newline() {
        let mut w = IndentWriter::new();
        w.write("hello ");
        w.write("world");
        w.newline();
        assert_eq!(w.finish(), "hello world\n");
    }

    #[test]
    fn test_nested_indent() {
        let mut w = IndentWriter::new();
        w.push_indent();
        w.push_indent();
        w.write_line("deep");
        w.pop_indent();
        w.write_line("shallow");
        assert_eq!(w.finish(), "    deep\n  shallow\n");
    }
}
