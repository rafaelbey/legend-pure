use crate::cursor::Cursor;
use legend_pure_parser_ast::type_ref::Package;
use legend_pure_parser_lexer::TokenKind;
use smol_str::SmolStr;

// ── Helpers ─────────────────────────────────────────────────────────────

pub(crate) fn split_package_name(pkg: &Package) -> (Option<Package>, SmolStr) {
    let name = SmolStr::new(pkg.name());
    (pkg.parent().cloned(), name)
}

pub(crate) fn unquote_string(s: &str) -> String {
    let inner = &s[1..s.len() - 1]; // strip surrounding quotes
    let mut result = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            #[allow(clippy::match_same_arms)] // Semantically distinct escapes
            match chars.next() {
                Some('\'') => result.push('\''),
                Some('\\') => result.push('\\'),
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some(other) => {
                    // Unrecognized escape — preserve verbatim
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'), // trailing backslash
            }
        } else {
            result.push(c);
        }
    }
    result
}

pub(crate) fn is_wildcard_ahead(cursor: &Cursor) -> bool {
    cursor.peek_kind_at(1) == TokenKind::Star
}
