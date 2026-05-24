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

//! Literal + variable lowering. The smallest, most isolated AST kind
//! — a clean home for `lower_literal` (every primitive form) and
//! `lower_variable` (`$name` reference), plus the date-literal
//! parsing helpers those depend on.
//!
//! Step 4 of the lowering encapsulation plan; first AST kind to
//! migrate from the `lower::mod` monolith into its own file.

use legend_pure_parser_ast::expression as ast_expr;
use smol_str::SmolStr;

use crate::types::{DateValue, ExprKind, ValueSpec};

use super::untyped;

/// Lowers an AST literal to a `ValueSpec`.
pub(super) fn lower_literal(lit: &ast_expr::Literal) -> Option<ValueSpec> {
    match lit {
        ast_expr::Literal::Integer(i) => Some(untyped(
            ExprKind::IntegerLiteral(i.value),
            i.source_info.clone(),
        )),
        ast_expr::Literal::Float(f) => Some(untyped(
            ExprKind::FloatLiteral(f.value),
            f.source_info.clone(),
        )),
        ast_expr::Literal::Decimal(d) => {
            let decimal = d.value.parse::<rust_decimal::Decimal>().ok()?;
            Some(untyped(
                ExprKind::DecimalLiteral(decimal),
                d.source_info.clone(),
            ))
        }
        ast_expr::Literal::String(s) => Some(untyped(
            ExprKind::StringLiteral(SmolStr::new(&s.value)),
            s.source_info.clone(),
        )),
        ast_expr::Literal::Boolean(b) => Some(untyped(
            ExprKind::BooleanLiteral(b.value),
            b.source_info.clone(),
        )),
        ast_expr::Literal::StrictDate(d) => {
            let dv = parse_strict_date(&d.value)?;
            Some(untyped(ExprKind::DateLiteral(dv), d.source_info.clone()))
        }
        ast_expr::Literal::DateTime(d) => {
            let dv = parse_datetime(&d.value)?;
            Some(untyped(ExprKind::DateLiteral(dv), d.source_info.clone()))
        }
        ast_expr::Literal::StrictTime(t) => {
            let dv = parse_strict_time(&t.value)?;
            Some(untyped(ExprKind::DateLiteral(dv), t.source_info.clone()))
        }
    }
}

/// Lowers a variable reference `$name`.
pub(super) fn lower_variable(var: &ast_expr::Variable) -> ValueSpec {
    untyped(
        ExprKind::Variable {
            name: var.name.clone(),
        },
        var.source_info.clone(),
    )
}

/// Parses `"2024-01-15"`, `"2024-01"`, `"2024"`, or `"%latest"` →
/// `DateValue::StrictDate` with appropriate precision (`month` / `day`
/// are `None` when the corresponding segment is missing) or
/// `DateValue::Latest` for the milestoning sentinel.
fn parse_strict_date(s: &str) -> Option<DateValue> {
    // %latest sentinel — recognise either with or without the leading `%`
    // so this is robust to whichever spelling the AST node carries.
    if s == "%latest" || s == "latest" {
        return Some(DateValue::Latest);
    }
    let s = s.strip_prefix('%').unwrap_or(s);
    let parts: Vec<&str> = s.split('-').collect();
    match parts.len() {
        1 => Some(DateValue::StrictDate {
            year: parts[0].parse().ok()?,
            month: None,
            day: None,
        }),
        2 => Some(DateValue::StrictDate {
            year: parts[0].parse().ok()?,
            month: Some(parts[1].parse().ok()?),
            day: None,
        }),
        3 => Some(DateValue::StrictDate {
            year: parts[0].parse().ok()?,
            month: Some(parts[1].parse().ok()?),
            day: Some(parts[2].parse().ok()?),
        }),
        _ => None,
    }
}

/// Parses `"2024-01-15T10:30:00"` (or with subseconds) → `DateValue::DateTime`.
fn parse_datetime(s: &str) -> Option<DateValue> {
    let s = s.strip_prefix('%').unwrap_or(s);
    let (date_part, time_part) = s.split_once('T')?;
    let date_parts: Vec<&str> = date_part.split('-').collect();
    if date_parts.len() != 3 {
        return None;
    }

    // Split the time component into `main_HH[:MM[:SS]]`, optional `.frac`,
    // and optional `±HHMM` TZ marker — each may be present or absent.
    let (time_body, tz_offset_minutes) = split_tz(time_part);
    let (time_main, subsec_str) = match time_body.split_once('.') {
        Some((main, frac)) => (main, frac),
        None => (time_body, ""),
    };

    let time_parts: Vec<&str> = time_main.split(':').collect();
    // Pure accepts hour-only datetimes — `%2015-04-15T17` parses to a
    // datetime with hour-only precision, missing minute/second slots.
    // Java Pure platform tests in essential/date/extract/year.pure rely
    // on this. Earlier our parser required MM:SS minimum and rejected
    // `T17` (length 1), so testYear et al hit "year: expected 1
    // argument(s), got 0" downstream.
    if time_parts.is_empty() {
        return None;
    }

    let (nanos, digits) = parse_subsecond_parts(subsec_str);
    let has_minutes = time_parts.len() >= 2;
    let has_seconds = time_parts.len() >= 3;

    Some(DateValue::DateTime {
        year: date_parts[0].parse().ok()?,
        month: date_parts[1].parse().ok()?,
        day: date_parts[2].parse().ok()?,
        hour: time_parts[0].parse().ok()?,
        minute: time_parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0),
        second: time_parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0),
        subsecond_nanos: nanos,
        subsecond_digits: digits,
        has_minutes,
        has_seconds,
        tz_offset_minutes,
    })
}

/// Parses `"10:30:00"` → `DateValue::StrictTime`.
fn parse_strict_time(s: &str) -> Option<DateValue> {
    let s = s.strip_prefix('%').unwrap_or(s);
    let (time_body, _tz) = split_tz(s);
    let (time_main, subsec_str) = match time_body.split_once('.') {
        Some((main, frac)) => (main, frac),
        None => (time_body, ""),
    };
    let parts: Vec<&str> = time_main.split(':').collect();
    if parts.len() < 2 {
        return None;
    }
    let (nanos, digits) = parse_subsecond_parts(subsec_str);
    Some(DateValue::StrictTime {
        hour: parts[0].parse().ok()?,
        minute: parts[1].parse().ok()?,
        second: parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0),
        subsecond_nanos: nanos,
        subsecond_digits: digits,
    })
}

/// Split a time body on its trailing `±HHMM` offset marker.
///
/// Returns `(body_without_tz, Some(offset_minutes))` when a marker is
/// present, else `(body, None)`. The `-` / `+` matching targets the LAST
/// occurrence so `-0500` doesn't get confused with fractional-second
/// separators (`.` takes precedence).
fn split_tz(s: &str) -> (&str, Option<i16>) {
    // Find the last `+` or `-` that has exactly 4 trailing digits (HHMM).
    for (idx, _) in s.char_indices().rev() {
        let byte = s.as_bytes()[idx];
        if byte == b'+' || byte == b'-' {
            let tail = &s[idx..];
            if tail.len() == 5 && tail.as_bytes()[1..].iter().all(u8::is_ascii_digit) {
                let sign: i16 = if byte == b'+' { 1 } else { -1 };
                let hh: i16 = tail[1..3].parse().unwrap_or(0);
                let mm: i16 = tail[3..5].parse().unwrap_or(0);
                return (&s[..idx], Some(sign * (hh * 60 + mm)));
            }
        }
    }
    (s, None)
}

/// Split a fractional-second string into `(nanoseconds, digits_present)`.
///
/// `digits_present` counts the source digits (1–9, capped at 9). `0`
/// means no fractional component was supplied. The nanos value is
/// padded to 9 digits on the right so 3-digit `.352` becomes
/// `352_000_000` nanoseconds.
fn parse_subsecond_parts(frac: &str) -> (i32, u8) {
    if frac.is_empty() {
        return (0, 0);
    }
    let trimmed: String = frac.chars().take(9).collect();
    #[allow(clippy::cast_possible_truncation)]
    let digits: u8 = trimmed.len() as u8;
    let mut padded = String::with_capacity(9);
    padded.push_str(&trimmed);
    while padded.len() < 9 {
        padded.push('0');
    }
    (padded.parse().unwrap_or(0), digits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DateValue;

    #[test]
    fn parse_strict_date_basic() {
        let dv = parse_strict_date("2024-01-15").unwrap();
        assert_eq!(
            dv,
            DateValue::StrictDate {
                year: 2024,
                month: Some(1),
                day: Some(15)
            }
        );
    }

    #[test]
    fn parse_strict_date_with_percent() {
        let dv = parse_strict_date("%2024-03-20").unwrap();
        assert_eq!(
            dv,
            DateValue::StrictDate {
                year: 2024,
                month: Some(3),
                day: Some(20)
            }
        );
    }

    #[test]
    fn parse_strict_date_year_only() {
        let dv = parse_strict_date("%2024").unwrap();
        assert_eq!(
            dv,
            DateValue::StrictDate {
                year: 2024,
                month: None,
                day: None,
            }
        );
    }

    #[test]
    fn parse_strict_date_year_month() {
        let dv = parse_strict_date("%2024-03").unwrap();
        assert_eq!(
            dv,
            DateValue::StrictDate {
                year: 2024,
                month: Some(3),
                day: None,
            }
        );
    }

    #[test]
    fn parse_datetime_basic() {
        let dv = parse_datetime("2024-01-15T10:30:00").unwrap();
        assert_eq!(
            dv,
            DateValue::DateTime {
                year: 2024,
                month: 1,
                day: 15,
                hour: 10,
                minute: 30,
                second: 0,
                subsecond_nanos: 0,
                subsecond_digits: 0,
                has_minutes: true,
                has_seconds: true,
                tz_offset_minutes: None,
            }
        );
    }

    #[test]
    fn parse_datetime_with_subseconds() {
        let dv = parse_datetime("%2024-01-15T10:30:45.123").unwrap();
        assert_eq!(
            dv,
            DateValue::DateTime {
                year: 2024,
                month: 1,
                day: 15,
                hour: 10,
                minute: 30,
                second: 45,
                subsecond_nanos: 123_000_000,
                subsecond_digits: 3,
                has_minutes: true,
                has_seconds: true,
                tz_offset_minutes: None,
            }
        );
    }

    #[test]
    fn parse_datetime_with_tz() {
        let dv = parse_datetime("%2024-01-15T10:30:45.352-0500").unwrap();
        assert_eq!(
            dv,
            DateValue::DateTime {
                year: 2024,
                month: 1,
                day: 15,
                hour: 10,
                minute: 30,
                second: 45,
                subsecond_nanos: 352_000_000,
                subsecond_digits: 3,
                has_minutes: true,
                has_seconds: true,
                tz_offset_minutes: Some(-300),
            }
        );
    }

    #[test]
    fn parse_datetime_minute_only() {
        let dv = parse_datetime("%2014-1-1T0:00+0000").unwrap();
        assert_eq!(
            dv,
            DateValue::DateTime {
                year: 2014,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                subsecond_nanos: 0,
                subsecond_digits: 0,
                has_minutes: true,
                has_seconds: false,
                tz_offset_minutes: Some(0),
            }
        );
    }

    #[test]
    fn parse_strict_time_basic() {
        let dv = parse_strict_time("10:30:00").unwrap();
        assert_eq!(
            dv,
            DateValue::StrictTime {
                hour: 10,
                minute: 30,
                second: 0,
                subsecond_nanos: 0,
                subsecond_digits: 0,
            }
        );
    }

    #[test]
    fn parse_strict_time_with_nanos() {
        let dv = parse_strict_time("%14:05:30.5").unwrap();
        assert_eq!(
            dv,
            DateValue::StrictTime {
                hour: 14,
                minute: 5,
                second: 30,
                subsecond_nanos: 500_000_000,
                subsecond_digits: 1,
            }
        );
    }

    #[test]
    fn parse_subsecond_parts_padding() {
        assert_eq!(parse_subsecond_parts("1"), (100_000_000, 1));
        assert_eq!(parse_subsecond_parts("12"), (120_000_000, 2));
        assert_eq!(parse_subsecond_parts("123"), (123_000_000, 3));
        assert_eq!(parse_subsecond_parts("123456789"), (123_456_789, 9));
        assert_eq!(parse_subsecond_parts(""), (0, 0));
    }
}
