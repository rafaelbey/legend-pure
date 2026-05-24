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

//! LSP-style ranged text edits to a single `.pure` file in a
//! [`Repo::Filesystem`] entry.
//!
//! Both the LSP server (`legend.applyEdit` execute-command) and the
//! MCP server (`apply_edit` tool) consume this module. It is the
//! single source of truth for:
//!
//! - the wire shape ([`TextEdit`] / [`Range`] / [`Position`]),
//! - UTF-16 code-unit → byte-offset translation,
//! - non-overlapping-edit validation,
//! - the in-memory mutation of [`Repo::Filesystem`]'s
//!   `OwnedSourceFile.content`.
//!
//! Callers are responsible for persisting the result to disk via
//! [`write_to_disk`] and for invalidating any downstream caches
//! (workspace recompile, dirty-file set).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use thiserror::Error;

use crate::repo::Repo;

/// 0-indexed source position. `character` counts **UTF-16 code units**
/// per LSP spec, not bytes or Unicode code points. The platform LSP
/// server negotiates UTF-16 as its position encoding
/// (`server.rs::initialize`), so this matches what the client sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// 0-indexed line number.
    pub line: u32,
    /// 0-indexed UTF-16 code-unit offset within the line.
    pub character: u32,
}

/// Half-open range `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    /// Inclusive start position.
    pub start: Position,
    /// Exclusive end position.
    pub end: Position,
}

/// A single LSP `TextEdit`: replace the text in `range` with `new_text`.
///
/// JSON wire shape matches the LSP `TextEdit` literal exactly —
/// `newText` (camelCase) for the field name so the LSP / MCP servers
/// can deserialize the same payload an IDE refactoring would send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEdit {
    /// Range of text in the source file to replace.
    pub range: Range,
    /// New text to insert. Empty string deletes the range.
    #[serde(rename = "newText")]
    pub new_text: String,
}

/// Result of a successful [`apply_text_edits`] call.
#[derive(Debug, Clone)]
pub struct AppliedEdit {
    /// The full post-edit file content. The in-memory
    /// `OwnedSourceFile.content` has already been overwritten with
    /// this value.
    pub new_content: String,
    /// Absolute on-disk path the [`Repo::Filesystem`] resolves the
    /// canonical URL to. Use this with [`write_to_disk`] to persist.
    pub disk_path: PathBuf,
    /// Canonical URL of the edited file (e.g. `/myproj/foo.pure`).
    /// Use this to key into per-file diagnostic maps and to mark the
    /// workspace dirty-file set after the edit.
    pub canonical_path: SmolStr,
}

/// Errors raised by [`apply_text_edits`] / [`write_to_disk`].
#[derive(Debug, Error)]
pub enum EditError {
    /// No [`Repo::Filesystem`] entry matched `path` by suffix.
    #[error("file not found in workspace: {path}")]
    FileNotInWorkspace {
        /// Path the caller asked to edit (canonical or absolute disk path).
        path: String,
    },
    /// The caller asked to edit a non-filesystem repo (embedded / purem).
    #[error("repo is not writable: kind={kind}")]
    RepoNotWritable {
        /// Human-readable repo kind that the match landed on.
        kind: &'static str,
    },
    /// The filesystem repo's [`Repo::source_root`] was `None`, so we
    /// can't synthesise a disk path to write to.
    #[error("filesystem repo has no source_root; cannot write to disk: {canonical}")]
    RepoMissingSourceRoot {
        /// Canonical URL that matched but lacked a `source_root`.
        canonical: String,
    },
    /// Two edits overlap.
    #[error("edits overlap: {a:?} vs {b:?}")]
    OverlappingEdits {
        /// First overlapping range (sorted-position order).
        a: Range,
        /// Second overlapping range.
        b: Range,
    },
    /// A position references a line past EOF or a character past the
    /// end of its line.
    #[error(
        "position out of range: line={line} character={character} (file has {file_lines} line(s))"
    )]
    OutOfRangePosition {
        /// 0-indexed line that was out of range.
        line: u32,
        /// 0-indexed character that was out of range.
        character: u32,
        /// Total lines in the file.
        file_lines: u32,
    },
    /// I/O failure while writing to disk.
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

/// Apply LSP-style ranged text edits to a single file in `repos`,
/// mutating the matching [`Repo::Filesystem`] entry's
/// `OwnedSourceFile.content` in place.
///
/// `file` is matched against the canonical URL prefix (e.g.
/// `/myproj/foo.pure`) **or** an absolute disk path whose tail
/// equals the canonical URL with the leading `/` stripped. This
/// mirrors the suffix-match strategy
/// [`crate::repo::Repo::Filesystem`] uses elsewhere
/// (`Workspace::snapshot_repos`, `Workspace::canonical_path_for`).
///
/// Edits are validated against the current content, sorted by
/// start position so overlap detection is a single linear sweep,
/// then applied in **descending** start order so earlier-byte
/// ranges stay valid as later ones rewrite the buffer.
///
/// # Errors
///
/// Returns [`EditError`] when:
/// - no filesystem repo carries the file,
/// - a matched entry is in an [`Repo::Embedded`] or [`Repo::Purem`] repo,
/// - the filesystem repo has no `source_root`,
/// - two edits overlap, or
/// - any position references a line past EOF or character past line end.
pub fn apply_text_edits(
    repos: &mut [Repo],
    file: &str,
    edits: &[TextEdit],
) -> Result<AppliedEdit, EditError> {
    // First pass: locate the matching Filesystem file across the slice.
    // Track the indices so the second pass can mutate the entry without
    // re-walking the (possibly large) repo list.
    let mut matched: Option<(usize, usize)> = None;
    let mut non_filesystem_match: Option<&'static str> = None;
    for (ri, repo) in repos.iter().enumerate() {
        match repo {
            Repo::Filesystem { files, .. } => {
                for (fi, f) in files.iter().enumerate() {
                    if path_matches(&f.path, file) {
                        matched = Some((ri, fi));
                        break;
                    }
                }
                if matched.is_some() {
                    break;
                }
            }
            Repo::Embedded { files, .. } => {
                for f in *files {
                    if path_matches(f.path, file) {
                        non_filesystem_match = Some("embedded");
                        break;
                    }
                }
            }
            Repo::Purem { manifests, .. } => {
                for (canonical, _) in *manifests {
                    if path_matches(canonical, file) {
                        non_filesystem_match = Some("purem");
                        break;
                    }
                }
            }
        }
        if matched.is_some() || non_filesystem_match.is_some() {
            break;
        }
    }
    if let Some(kind) = non_filesystem_match
        && matched.is_none()
    {
        return Err(EditError::RepoNotWritable { kind });
    }
    let (ri, fi) = matched.ok_or_else(|| EditError::FileNotInWorkspace {
        path: file.to_string(),
    })?;

    // Extract source_root + canonical path + content. Doing this in a
    // narrow scope keeps the borrow short so the later mutable borrow
    // is straightforward.
    let (canonical_path, disk_path, content) = {
        let Repo::Filesystem {
            files, source_root, ..
        } = &repos[ri]
        else {
            // Can't reach: `matched` only set in the Filesystem arm above.
            return Err(EditError::RepoNotWritable { kind: "(internal)" });
        };
        let f = &files[fi];
        let canonical = SmolStr::new(&f.path);
        let root = source_root
            .as_ref()
            .ok_or_else(|| EditError::RepoMissingSourceRoot {
                canonical: f.path.clone(),
            })?;
        // Compute the disk path from source_root + (canonical -
        // "/{repo_prefix}/" tail). The canonical is "/{repo_name}/{rel}";
        // source_root already points to "{...}/{repo_name}/", so we
        // join the part **after** the repo segment to avoid pasting the
        // repo name twice. The repo prefix is the canonical's leading
        // "/name" segment; strip it then strip the separator.
        let rel = canonical_relative_to_root(&f.path);
        let disk = root.join(rel);
        (canonical, disk, f.content.clone())
    };

    // Validate + apply edits against `content`.
    let new_content = compute_new_content(&content, edits)?;

    // Mutate the in-memory file.
    if let Repo::Filesystem { files, .. } = &mut repos[ri] {
        files[fi].content.clone_from(&new_content);
    }

    Ok(AppliedEdit {
        new_content,
        disk_path,
        canonical_path,
    })
}

/// Persist [`AppliedEdit::new_content`] to [`AppliedEdit::disk_path`]
/// with a single [`std::fs::write`].
///
/// Kept separate from [`apply_text_edits`] so a caller that wants
/// to defer or batch disk writes (LSP IDE-side overlay, dry-run
/// preview) can do so.
///
/// # Errors
///
/// Returns [`EditError::IoError`] on any underlying I/O failure.
pub fn write_to_disk(applied: &AppliedEdit) -> Result<(), EditError> {
    std::fs::write(&applied.disk_path, &applied.new_content)?;
    Ok(())
}

/// Check whether the [`OwnedSourceFile::path`] (a canonical URL) matches
/// the caller-supplied `file` (canonical URL **or** absolute disk path).
///
/// - Exact canonical match: `path == file`.
/// - Suffix match: `file` ends with `path` with its leading `/`
///   stripped — e.g. `/abs/proj/myproj/foo.pure` ends with
///   `myproj/foo.pure`.
///
/// Mirrors the existing match strategy in
/// `Workspace::snapshot_repos` / `Workspace::canonical_path_for`.
///
/// TODO(windows): the suffix branch assumes the caller-supplied disk
/// path uses `/` separators. Real Windows paths (`C:\proj\foo.pure`)
/// will not match against a canonical URL (`/proj/foo.pure`) because
/// `ends_with` is byte-level on `str`. The boundary check accepts
/// `\` only at the join point, not in the body of the tail. The
/// upstream `Workspace::snapshot_repos` shares this limitation —
/// fix both together when we add Windows CI.
fn path_matches(canonical_path: &str, file: &str) -> bool {
    if canonical_path == file {
        return true;
    }
    let tail = canonical_path.trim_start_matches('/');
    if tail.is_empty() {
        return false;
    }
    if file.ends_with(tail) {
        // Boundary check: the character immediately before `tail` in
        // `file` must be a path separator (or the match is the full
        // string). Without this, `foo.pure` would match a canonical
        // URL of `/oo.pure`.
        let prefix_len = file.len() - tail.len();
        if prefix_len == 0 {
            return true;
        }
        let preceding = file.as_bytes()[prefix_len - 1];
        return preceding == b'/' || preceding == b'\\';
    }
    false
}

/// Drop the leading "/{repo_name}/" from a canonical URL so the
/// remainder can be joined onto a `source_root` (which itself already
/// points at the repo's root directory).
fn canonical_relative_to_root(canonical: &str) -> &str {
    // canonical is "/{repo_name}/{rel}". Strip leading '/' then strip
    // up to and including the next '/'.
    let no_leading = canonical.trim_start_matches('/');
    no_leading.split_once('/').map_or("", |(_, rest)| rest)
}

/// Validate the edits, sort them descending, and apply each replacement.
///
/// Splitting this out keeps [`apply_text_edits`] focused on the
/// repo-lookup + in-memory mutation, while this fn owns the actual
/// content algebra.
fn compute_new_content(content: &str, edits: &[TextEdit]) -> Result<String, EditError> {
    if edits.is_empty() {
        return Ok(content.to_string());
    }

    // Line-start byte offsets keyed by line index. The Nth entry is
    // the byte offset where line N starts in `content`. There's an
    // implicit final entry equal to `content.len()` for the EOF
    // sentinel — accessed via `line_starts.len()` checks below.
    let line_starts = compute_line_starts(content);
    let line_count: u32 = u32::try_from(line_starts.len()).unwrap_or(u32::MAX);

    // Resolve every edit's range to a byte-offset pair up-front so
    // overlap detection + sorting are O(N log N) on simple integers.
    let mut resolved: Vec<(usize, usize, &str)> = Vec::with_capacity(edits.len());
    for e in edits {
        let start_byte = position_to_byte(content, &line_starts, line_count, e.range.start)?;
        let end_byte = position_to_byte(content, &line_starts, line_count, e.range.end)?;
        // A start > end is degenerate; report it as overlap so the
        // caller gets a single error class to handle.
        if start_byte > end_byte {
            return Err(EditError::OverlappingEdits {
                a: e.range,
                b: e.range,
            });
        }
        resolved.push((start_byte, end_byte, e.new_text.as_str()));
    }

    // Sort ascending by start, then check for overlap. Two edits
    // overlap iff `a.end > b.start` (after sort) — a touching edit
    // (`a.end == b.start`) is allowed.
    let mut order: Vec<usize> = (0..resolved.len()).collect();
    order.sort_by_key(|&i| (resolved[i].0, resolved[i].1));
    for window in order.windows(2) {
        let (i, j) = (window[0], window[1]);
        if resolved[i].1 > resolved[j].0 {
            return Err(EditError::OverlappingEdits {
                a: edits[i].range,
                b: edits[j].range,
            });
        }
    }

    // Apply edits in descending start order so earlier-byte ranges
    // stay valid. Build the result by mutating a `String` clone of the
    // input.
    let mut out = content.to_string();
    for &i in order.iter().rev() {
        let (s, e, t) = resolved[i];
        out.replace_range(s..e, t);
    }
    Ok(out)
}

/// Compute the byte offset of the start of each line in `content`.
/// `content` line N starts at byte `result[N]`. Empty files still get
/// one entry (`[0]`).
fn compute_line_starts(content: &str) -> Vec<usize> {
    let mut out = Vec::with_capacity(content.len() / 32 + 1);
    out.push(0);
    let bytes = content.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            out.push(i + 1);
        }
    }
    out
}

/// Translate an LSP [`Position`] (0-indexed line, UTF-16 code units
/// for `character`) into a UTF-8 byte offset in `content`.
fn position_to_byte(
    content: &str,
    line_starts: &[usize],
    line_count: u32,
    pos: Position,
) -> Result<usize, EditError> {
    if pos.line as usize >= line_starts.len() {
        // Edge case: a position at `line == line_count` with
        // `character == 0` is a common LSP idiom for "end of file"
        // (matches whole-file ranges). Treat it as content.len().
        if pos.line == line_count && pos.character == 0 {
            return Ok(content.len());
        }
        return Err(EditError::OutOfRangePosition {
            line: pos.line,
            character: pos.character,
            file_lines: line_count,
        });
    }
    let line_start_byte = line_starts[pos.line as usize];
    let line_end_byte = if (pos.line as usize) + 1 < line_starts.len() {
        // Subtract one to exclude the trailing '\n' from the line's
        // text range — the position-encoding walk shouldn't try to
        // index past the newline.
        line_starts[pos.line as usize + 1] - 1
    } else {
        content.len()
    };
    let line_text = &content[line_start_byte..line_end_byte];
    let utf16_offset = pos.character as usize;
    // Walk the line, counting UTF-16 code units, until we hit
    // `utf16_offset`. A char's UTF-16 length is 1 for BMP, 2 for
    // surrogate-pair (any `char` whose value is > U+FFFF).
    let mut accumulated_u16 = 0usize;
    let mut byte_in_line = 0usize;
    for ch in line_text.chars() {
        if accumulated_u16 == utf16_offset {
            break;
        }
        let u16_len = ch.len_utf16();
        if accumulated_u16 + u16_len > utf16_offset {
            // The requested UTF-16 offset lands in the middle of a
            // surrogate pair. Treat it as out-of-range — LSP clients
            // should never send this.
            return Err(EditError::OutOfRangePosition {
                line: pos.line,
                character: pos.character,
                file_lines: line_count,
            });
        }
        accumulated_u16 += u16_len;
        byte_in_line += ch.len_utf8();
    }
    if accumulated_u16 != utf16_offset {
        // We walked off the end of the line without reaching the
        // requested offset.
        return Err(EditError::OutOfRangePosition {
            line: pos.line,
            character: pos.character,
            file_lines: line_count,
        });
    }
    Ok(line_start_byte + byte_in_line)
}
