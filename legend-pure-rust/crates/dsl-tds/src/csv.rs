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

//! Shared CSV parser + per-column type inference for TDS literals.
//!
//! Single source of truth for both the compile-time `#TDS\n…\n#`
//! lowerer and the runtime `meta::pure::metamodel::relation::stringToTDS`
//! native. Identical input → identical [`ParsedTDS`] output. The two
//! consumers may use different slices of the result (the lowerer reads
//! [`ParsedTDS::columns`] only, to build a typed `RelationType<…>`; the
//! runtime stores the whole struct on the TDS instance), but they both
//! call [`parse_and_infer`].
//!
//! Mirrors Java's `org.finos.legend.pure.m2.inlinedsl.tds.TDSExtension`,
//! which delegates to Deephaven CSV. We do not pull in Deephaven (no
//! Rust port); a hand-rolled minimal reader + classifier covers the
//! cases real engine source uses. See `feedback_no_tactical_hacks` —
//! when a real engine source surfaces a CSV edge case we don't handle
//! (embedded newlines in quoted strings, BOM, etc.), add coverage
//! rather than papering over.
//!
//! # Per-column type inference precedence
//!
//! For each column, classify by scanning every non-empty cell in the
//! column's data. The first type that ALL non-empty cells satisfy
//! (in this order) wins:
//!
//! 1. **`Integer`** — every non-empty cell parses as `^-?\d+$`.
//! 2. **`Decimal`** — every non-empty cell ends with `D`/`d`
//!    (`^-?\d+(\.\d+)?[Dd]$`).
//! 3. **`Float`** — every non-empty cell parses as a decimal-bearing
//!    number (`^-?\d+(\.\d+)?$`, allows trailing `.\d+`). Integers
//!    promote up if the column has any float — but the precedence
//!    above means we only land here if Integer didn't match.
//! 4. **`Boolean`** — every non-empty cell is `true` or `false`
//!    (case-insensitive).
//! 5. **`DateTime`** — `YYYY-MM-DDTHH:MM:SS[.fraction][+ZZZZ|Z]`.
//! 6. **`StrictDate`** — `YYYY-MM-DD`.
//! 7. **`String`** — fallback. Any column whose cells include
//!    quoted strings (single or double) is forced to `String`
//!    immediately.
//!
//! Multiplicity: `[1]` if every cell in the column is non-empty;
//! `[0..1]` otherwise. All-empty column → `String[0..1]`.

use legend_pure_parser_pure::types::Multiplicity;
use smol_str::SmolStr;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Parsed-and-typed TDS data. The single output of [`parse_and_infer`],
/// shared by the compile-time lowerer and the `stringToTDS` runtime
/// native.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedTDS {
    /// The canonical CSV string, exactly as it was passed in (post-
    /// trimming of any leading/trailing blank lines). Reconstructed
    /// here so the compile-time lowerer has a single string to embed
    /// as the `stringToTDS` argument.
    pub csv: String,
    /// One entry per column, in declaration order.
    pub columns: Vec<ParsedColumn>,
    /// Materialised cell values. Outer vec is rows, inner vec is
    /// cells in column order. `None` denotes an empty cell.
    pub rows: Vec<Vec<Option<TypedCell>>>,
}

/// A single TDS column with its inferred (or overridden) type and
/// multiplicity.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedColumn {
    /// Column name, exactly as it appeared in the header line
    /// (whitespace-trimmed, quotes stripped).
    pub name: SmolStr,
    /// Inferred or overridden Pure-side primitive type.
    pub type_tag: ColumnType,
    /// Inferred or overridden multiplicity. `[1]` if no empty cells,
    /// else `[0..1]`.
    pub multiplicity: Multiplicity,
}

/// Pure-side type a TDS column resolves to. The seven primitives are
/// recognised by data-driven inference; `Other` carries an arbitrary
/// class reference parsed from a `name:Pkg::Class` header annotation
/// (e.g. `payload:meta::pure::metamodel::variant::Variant`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnType {
    /// `Integer` — whole-number literal.
    Integer,
    /// `Float` — decimal-bearing number.
    Float,
    /// `Decimal` — fixed-point literal with `D`/`d` suffix.
    Decimal,
    /// `Boolean` — `true` or `false`.
    Boolean,
    /// `String` — quoted, or fallback for unparseable columns.
    String,
    /// `StrictDate` — `YYYY-MM-DD`.
    StrictDate,
    /// `DateTime` — `YYYY-MM-DDTHH:MM:SS[.fraction][+ZZZZ|Z]`.
    DateTime,
    /// Arbitrary class type from an explicit header annotation. The
    /// inferer never produces this — it can only be set via
    /// [`ColumnOverride`]. Cell values for such columns are stored as
    /// [`TypedCell::String`] (raw text); the runtime is expected to
    /// reconstruct the typed value from that string.
    Other {
        /// `::`-joined package path (e.g. `meta::pure::metamodel::variant`),
        /// or `None` for an unqualified class name.
        package: Option<SmolStr>,
        /// Bare class name (e.g. `Variant`).
        name: SmolStr,
    },
}

impl ColumnType {
    /// Pure-side bare type name for use in the lowered
    /// `RelationType<…>` AST.
    #[must_use]
    pub fn pure_type_name(&self) -> &str {
        match self {
            ColumnType::Integer => "Integer",
            ColumnType::Float => "Float",
            ColumnType::Decimal => "Decimal",
            ColumnType::Boolean => "Boolean",
            ColumnType::String => "String",
            ColumnType::StrictDate => "StrictDate",
            ColumnType::DateTime => "DateTime",
            ColumnType::Other { name, .. } => name.as_str(),
        }
    }

    /// `::`-joined package path that qualifies the type name, when the
    /// column carries an `Other` annotation with a package prefix.
    /// `None` for primitives (which live in `meta::pure::metamodel` and
    /// are auto-imported) and for bare-name `Other` annotations.
    #[must_use]
    pub fn pure_type_package(&self) -> Option<&str> {
        match self {
            ColumnType::Other {
                package: Some(p), ..
            } => Some(p.as_str()),
            _ => None,
        }
    }
}

/// A typed cell value. Variant matches the column's `ColumnType`.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedCell {
    /// `Integer` value.
    Integer(i64),
    /// `Float` value (parsed via `f64::from_str`).
    Float(f64),
    /// `Decimal` — kept as the source-form string with `D` suffix
    /// for downstream `rust_decimal::Decimal::from_str`.
    Decimal(SmolStr),
    /// `Boolean` value.
    Boolean(bool),
    /// `String` value with surrounding quotes stripped and `\\<quote>`
    /// escapes resolved.
    String(SmolStr),
    /// `StrictDate` — kept as the source-form `YYYY-MM-DD` string.
    StrictDate(SmolStr),
    /// `DateTime` — kept as the source-form ISO-8601-ish string.
    DateTime(SmolStr),
}

/// Per-column type/multiplicity override. The compile-time lowerer
/// passes one entry per `name:Type[mult]` column declaration parsed
/// from `#TDS\n …\n#`; the runtime native passes none.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColumnOverride {
    /// Explicit Pure type from `name:Type` syntax. `None` → infer.
    pub type_tag: Option<ColumnType>,
    /// Explicit multiplicity from `name:Type[mult]`. `None` → infer.
    pub multiplicity: Option<Multiplicity>,
}

/// Error returned when CSV parsing or type classification fails on
/// some structural rule (e.g. row arity mismatch). Inference itself
/// is total — it always falls back to `String` — so this only fires
/// for malformed input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvError {
    /// Human-readable description.
    pub message: String,
    /// 1-based line in the source CSV string where the error was
    /// detected.
    pub line: usize,
    /// 1-based column (character offset) where the error was detected.
    pub column: usize,
}

impl std::fmt::Display for CsvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (line {}, col {})",
            self.message, self.line, self.column
        )
    }
}

impl std::error::Error for CsvError {}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Parse a TDS CSV string and infer per-column types + multiplicities.
///
/// `csv` is expected to contain a header line (column names) followed
/// by zero or more data rows, separated by `\n`. Cells are
/// comma-separated. Quoted strings (single `'…'` or double `"…"`)
/// preserve internal commas; `\\` escapes the quote char inside a
/// quoted string.
///
/// `overrides` is positional: `overrides[i]` (when present) overrides
/// the `i`-th column's inferred type and/or multiplicity. Empty or
/// shorter `overrides` means columns past its length are fully inferred.
///
/// # Errors
///
/// Returns [`CsvError`] when:
/// - The header line is missing (CSV is empty after trimming).
/// - A data row has a cell count different from the header's column
///   count.
/// - A quoted string is unterminated.
pub fn parse_and_infer(csv: &str, overrides: &[ColumnOverride]) -> Result<ParsedTDS, CsvError> {
    // Trim leading/trailing blank lines but preserve internal structure.
    let trimmed = csv.trim_matches(|c: char| c == '\n' || c == '\r');

    let mut lines = split_lines(trimmed);
    if lines.is_empty() {
        return Err(CsvError {
            message: "TDS CSV is empty (no header line)".into(),
            line: 1,
            column: 1,
        });
    }

    let header_line = lines.remove(0);
    let header_cells = parse_csv_line(header_line.text, header_line.line)?;
    let column_count = header_cells.len();

    // Parse all data rows up front so we can scan column-major during
    // inference. Each entry preserves its source line for downstream
    // error positioning.
    let mut data_rows: Vec<DataRow> = Vec::with_capacity(lines.len());
    for raw_line in lines {
        let cells = parse_csv_line(raw_line.text, raw_line.line)?;
        if cells.len() != column_count {
            return Err(CsvError {
                message: format!(
                    "TDS row has {} cell(s); expected {} (one per declared column)",
                    cells.len(),
                    column_count
                ),
                line: raw_line.line,
                column: 1,
            });
        }
        data_rows.push(DataRow {
            line: raw_line.line,
            cells,
        });
    }

    // Per-column inference: walk every cell in the column to classify.
    let mut columns = Vec::with_capacity(column_count);
    for (idx, header) in header_cells.iter().enumerate() {
        let override_entry = overrides.get(idx).cloned().unwrap_or_default();
        let column_cells: Vec<&RawCell> = data_rows.iter().map(|row| &row.cells[idx]).collect();
        let inferred = infer_column(&column_cells);
        let type_tag = override_entry.type_tag.clone().unwrap_or(inferred.0);
        let multiplicity = override_entry.multiplicity.unwrap_or(inferred.1);
        columns.push(ParsedColumn {
            name: SmolStr::new(header.canonical_name()),
            type_tag,
            multiplicity,
        });
    }

    // Materialise cells per the chosen column types. Type-mismatch
    // errors (non-empty cell that doesn't parse against the declared
    // type) surface as `CsvError`s positioned at the offending row.
    let mut rows: Vec<Vec<Option<TypedCell>>> = Vec::with_capacity(data_rows.len());
    for (row_idx, row) in data_rows.iter().enumerate() {
        let mut typed_row = Vec::with_capacity(column_count);
        for (idx, cell) in row.cells.iter().enumerate() {
            match materialise_cell(cell, &columns[idx].type_tag) {
                Ok(value) => typed_row.push(value),
                Err(reason) => {
                    return Err(CsvError {
                        message: format!(
                            "TDS column '{}' is declared `{}` but the value {:?} on row {} is not a valid {}: {}",
                            columns[idx].name,
                            columns[idx].type_tag.pure_type_name(),
                            cell.raw,
                            row_idx + 1,
                            columns[idx].type_tag.pure_type_name(),
                            reason,
                        ),
                        line: row.line,
                        column: idx + 1,
                    });
                }
            }
        }
        rows.push(typed_row);
    }

    // Multiplicity validation: when a column carries an explicit `[1]`
    // (or `[1..*]`), every cell must be non-empty. Inferred
    // multiplicities are by construction consistent with the data, so
    // this only fires when an override forces a tighter bound than the
    // CSV supports.
    for (col_idx, col) in columns.iter().enumerate() {
        if !multiplicity_requires_nonempty(&col.multiplicity) {
            continue;
        }
        if let Some((row_idx, row)) = data_rows
            .iter()
            .enumerate()
            .find(|(_, r)| r.cells[col_idx].is_empty())
        {
            return Err(CsvError {
                message: format!(
                    "TDS column '{}' is declared `{}` (`{}`) but row {} cell is empty",
                    col.name,
                    col.type_tag.pure_type_name(),
                    multiplicity_display(&col.multiplicity),
                    row_idx + 1,
                ),
                line: row.line,
                column: col_idx + 1,
            });
        }
    }

    Ok(ParsedTDS {
        csv: trimmed.to_string(),
        columns,
        rows,
    })
}

/// One parsed CSV data row paired with its source-line number.
struct DataRow {
    line: usize,
    cells: Vec<RawCell>,
}

/// Returns `true` when `m` requires at least one non-empty cell per
/// row (`[1]`, `[1..*]`, `[1..n]`). `Variable` (parameterised) is
/// treated permissively — the binding is unknown at parse time.
fn multiplicity_requires_nonempty(m: &Multiplicity) -> bool {
    use Multiplicity::{OneOrMany, PureOne, Range, Variable, ZeroOrMany, ZeroOrOne};
    match m {
        PureOne | OneOrMany => true,
        Range { lower, .. } => *lower >= 1,
        ZeroOrOne | ZeroOrMany | Variable(_) => false,
    }
}

/// Render a [`Multiplicity`] in source-form for error messages
/// (`1`, `0..1`, `*`, `1..*`, `2..5`).
fn multiplicity_display(m: &Multiplicity) -> String {
    use Multiplicity::{OneOrMany, PureOne, Range, Variable, ZeroOrMany, ZeroOrOne};
    match m {
        PureOne => "1".into(),
        ZeroOrOne => "0..1".into(),
        ZeroOrMany => "*".into(),
        OneOrMany => "1..*".into(),
        Range {
            lower,
            upper: Some(u),
        } => format!("{lower}..{u}"),
        Range { lower, upper: None } => format!("{lower}..*"),
        Variable(name) => name.to_string(),
    }
}

// ---------------------------------------------------------------------------
// CSV parsing — minimal hand-rolled reader
// ---------------------------------------------------------------------------

/// A raw cell — the substring of source between two commas, with
/// quoting preserved so the inferrer can distinguish `'1'` (quoted →
/// String) from `1` (numeric).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawCell {
    /// The exact source-form text including any surrounding quotes.
    raw: String,
    /// `true` iff the cell was wrapped in `'…'` or `"…"`.
    quoted: bool,
}

impl RawCell {
    /// Returns the cell text with leading/trailing whitespace trimmed
    /// and surrounding quotes stripped. Used for column-name
    /// extraction and type-pattern matching against the bare value.
    fn canonical_name(&self) -> &str {
        let trimmed = self.raw.trim();
        if self.quoted && trimmed.len() >= 2 {
            &trimmed[1..trimmed.len() - 1]
        } else {
            trimmed
        }
    }

    /// The trimmed value of the cell — quotes stripped — for type
    /// classification and parsing. Empty trimmed value (`""`) signals
    /// a NULL/empty cell.
    fn value(&self) -> &str {
        let trimmed = self.raw.trim();
        if self.quoted && trimmed.len() >= 2 {
            &trimmed[1..trimmed.len() - 1]
        } else {
            trimmed
        }
    }

    /// `true` if this cell carries no value. Bare cells with no
    /// non-whitespace text are empty; quoted cells whose content is
    /// likewise empty (`''`, `""`) are also treated as empty so they
    /// can mark a null in the standard CSV convention. A non-empty
    /// quoted cell (`'foo'`) is not empty.
    fn is_empty(&self) -> bool {
        if self.quoted {
            // Strip surrounding quotes and check the inner text.
            self.value().trim().is_empty()
        } else {
            self.raw.trim().is_empty()
        }
    }
}

/// A CSV input line with its 1-based source line number.
struct InputLine<'a> {
    text: &'a str,
    line: usize,
}

fn split_lines(csv: &str) -> Vec<InputLine<'_>> {
    let mut out = Vec::new();
    for (idx, text) in csv.split('\n').enumerate() {
        // Strip trailing `\r` for `\r\n` line endings.
        let text = text.strip_suffix('\r').unwrap_or(text);
        out.push(InputLine {
            text,
            line: idx + 1,
        });
    }
    // Discard a trailing empty line that comes from a `\n`-terminated
    // input — common in source-form CSVs.
    if out.last().is_some_and(|l| l.text.is_empty()) {
        out.pop();
    }
    out
}

/// Split a single CSV line on commas, respecting `'…'` and `"…"`
/// quoted segments. Commas inside quoted strings are not separators.
/// Backslash-escapes (`\\'` / `\\"`) inside quoted strings are
/// preserved verbatim in `RawCell::raw` for downstream materialisation.
fn parse_csv_line(line: &str, line_no: usize) -> Result<Vec<RawCell>, CsvError> {
    let mut cells = Vec::new();
    let mut chars = line.char_indices().peekable();
    let mut start: usize = 0;
    let mut in_quote: Option<char> = None;
    let mut had_quote = false;

    while let Some((idx, ch)) = chars.next() {
        if let Some(qc) = in_quote {
            // Skip escaped quote.
            if ch == '\\' {
                chars.next(); // consume escaped char
                continue;
            }
            if ch == qc {
                in_quote = None;
            }
        } else {
            match ch {
                '\'' | '"' => {
                    in_quote = Some(ch);
                    had_quote = true;
                }
                ',' => {
                    let raw = line[start..idx].to_string();
                    cells.push(RawCell {
                        raw,
                        quoted: had_quote,
                    });
                    start = idx + 1; // skip the comma byte
                    had_quote = false;
                }
                _ => {}
            }
        }
    }

    if let Some(opening) = in_quote {
        return Err(CsvError {
            message: format!(
                "Unterminated quoted string in TDS row (opening {opening} not closed)"
            ),
            line: line_no,
            column: start + 1,
        });
    }

    // Push the final cell.
    let raw = line[start..].to_string();
    cells.push(RawCell {
        raw,
        quoted: had_quote,
    });
    Ok(cells)
}

// ---------------------------------------------------------------------------
// Per-column type inference
// ---------------------------------------------------------------------------

fn infer_column(cells: &[&RawCell]) -> (ColumnType, Multiplicity) {
    let any_empty = cells.iter().any(|c| c.is_empty());
    let multiplicity = if any_empty {
        Multiplicity::ZeroOrOne
    } else {
        Multiplicity::PureOne
    };

    // Quoted cells force String; column type can't be anything else.
    if cells.iter().any(|c| c.quoted) {
        return (ColumnType::String, multiplicity);
    }

    let non_empty: Vec<&str> = cells
        .iter()
        .filter(|c| !c.is_empty())
        .map(|c| c.value())
        .collect();

    // All-empty column: default String[0..1].
    if non_empty.is_empty() {
        return (ColumnType::String, multiplicity);
    }

    // Apply precedence: each predicate must match every non-empty cell.
    if non_empty.iter().all(|s| is_integer_literal(s)) {
        return (ColumnType::Integer, multiplicity);
    }
    if non_empty.iter().all(|s| is_decimal_literal(s)) {
        return (ColumnType::Decimal, multiplicity);
    }
    if non_empty.iter().all(|s| is_float_literal(s)) {
        return (ColumnType::Float, multiplicity);
    }
    if non_empty.iter().all(|s| is_boolean_literal(s)) {
        return (ColumnType::Boolean, multiplicity);
    }
    if non_empty.iter().all(|s| is_datetime_literal(s)) {
        return (ColumnType::DateTime, multiplicity);
    }
    if non_empty.iter().all(|s| is_strict_date_literal(s)) {
        return (ColumnType::StrictDate, multiplicity);
    }
    (ColumnType::String, multiplicity)
}

fn is_integer_literal(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    !body.is_empty() && body.chars().all(|c| c.is_ascii_digit())
}

fn is_decimal_literal(s: &str) -> bool {
    let body = s.strip_suffix(['D', 'd']);
    let Some(body) = body else { return false };
    let body = body.strip_prefix('-').unwrap_or(body);
    if body.is_empty() {
        return false;
    }
    let mut parts = body.splitn(2, '.');
    let int_part = parts.next().unwrap_or("");
    let frac_part = parts.next().unwrap_or("");
    !int_part.is_empty()
        && int_part.chars().all(|c| c.is_ascii_digit())
        && (frac_part.is_empty() || frac_part.chars().all(|c| c.is_ascii_digit()))
}

fn is_float_literal(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    if body.is_empty() {
        return false;
    }
    let mut parts = body.splitn(2, '.');
    let int_part = parts.next().unwrap_or("");
    let frac_part = parts.next().unwrap_or("");
    if int_part.is_empty() || !int_part.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    // Float must have a fractional part — otherwise Integer would
    // have matched. `1.` is still a float (trailing dot allowed by
    // most parsers, including Pure's).
    if frac_part.is_empty() {
        return false;
    }
    frac_part.chars().all(|c| c.is_ascii_digit())
}

fn is_boolean_literal(s: &str) -> bool {
    s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("false")
}

fn is_strict_date_literal(s: &str) -> bool {
    // YYYY-MM-DD — exactly 10 chars, ASCII digits and hyphens.
    if s.len() != 10 {
        return false;
    }
    let bytes = s.as_bytes();
    bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

fn is_datetime_literal(s: &str) -> bool {
    // YYYY-MM-DDTHH:MM:SS, optionally followed by `.fraction`,
    // optionally followed by `+HHMM` / `-HHMM` / `Z`.
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return false;
    }
    if !is_strict_date_literal(&s[..10]) {
        return false;
    }
    if bytes[10] != b'T' {
        return false;
    }
    if !(bytes[11..13].iter().all(u8::is_ascii_digit)
        && bytes[13] == b':'
        && bytes[14..16].iter().all(u8::is_ascii_digit)
        && bytes[16] == b':'
        && bytes[17..19].iter().all(u8::is_ascii_digit))
    {
        return false;
    }
    let mut tail = &s[19..];
    if let Some(after_dot) = tail.strip_prefix('.') {
        let frac_end = after_dot
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_dot.len());
        if frac_end == 0 {
            return false;
        }
        tail = &after_dot[frac_end..];
    }
    if tail.is_empty() || tail == "Z" {
        return true;
    }
    // ±HHMM
    if (tail.starts_with('+') || tail.starts_with('-')) && tail.len() == 5 {
        return tail.as_bytes()[1..].iter().all(u8::is_ascii_digit);
    }
    false
}

// ---------------------------------------------------------------------------
// Cell materialisation
// ---------------------------------------------------------------------------

/// Materialise a single cell. Returns:
///
/// - `Ok(None)` when the cell is empty (whitespace-only or missing).
/// - `Ok(Some(typed))` when the cell parses against `type_tag`.
/// - `Err(reason)` when the cell text is non-empty but doesn't fit
///   `type_tag` (e.g. `"hello"` declared `Integer`, or `"yes"`
///   declared `Boolean`). The `reason` is a short description suitable
///   for embedding in a positioned diagnostic.
///
/// Decimal / `StrictDate` / `DateTime` / `String` / `Other` accept any
/// non-empty text — typed parsing of those forms happens later (the
/// compiler keeps the source-form string and the runtime decides how
/// to interpret it).
fn materialise_cell(
    cell: &RawCell,
    type_tag: &ColumnType,
) -> Result<Option<TypedCell>, &'static str> {
    if cell.is_empty() {
        return Ok(None);
    }
    let v = cell.value();
    Ok(Some(match type_tag {
        ColumnType::Integer => v
            .parse::<i64>()
            .map(TypedCell::Integer)
            .map_err(|_| "expected a whole-number Integer literal")?,
        ColumnType::Float => v
            .parse::<f64>()
            .map(TypedCell::Float)
            .map_err(|_| "expected a Float literal (e.g. `1.5`, `-3.14`)")?,
        ColumnType::Decimal => TypedCell::Decimal(SmolStr::new(v)),
        ColumnType::Boolean => {
            if v.eq_ignore_ascii_case("true") {
                TypedCell::Boolean(true)
            } else if v.eq_ignore_ascii_case("false") {
                TypedCell::Boolean(false)
            } else {
                return Err("expected `true` or `false`");
            }
        }
        ColumnType::String => TypedCell::String(unescape_string(v)),
        ColumnType::StrictDate => TypedCell::StrictDate(SmolStr::new(v)),
        ColumnType::DateTime => TypedCell::DateTime(SmolStr::new(v)),
        // Arbitrary class column: store the raw text. The runtime
        // reconstructs the typed value from that string at execution
        // time (e.g. `Variant` parses its own JSON-bearing payload).
        ColumnType::Other { .. } => TypedCell::String(unescape_string(v)),
    }))
}

fn unescape_string(s: &str) -> SmolStr {
    if !s.contains('\\') {
        return SmolStr::new(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(next) = chars.next()
        {
            out.push(next);
        } else {
            out.push(c);
        }
    }
    SmolStr::new(out)
}
