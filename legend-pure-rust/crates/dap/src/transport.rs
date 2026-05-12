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

//! `Content-Length:`-framed JSON over stdio.
//!
//! DAP uses the same envelope as LSP — one or more headers separated
//! by `\r\n`, a blank `\r\n`, then exactly `Content-Length` bytes of
//! JSON. We hand-roll the read side (tokio's `AsyncBufRead` is fine
//! but we want to keep tokio-fs out of the picture for the simple
//! single-threaded transport loop).

use crate::error::DapError;
use std::io::{BufRead, BufReader, Read, Write};

/// Read one DAP frame from a buffered reader.
///
/// Blocks until a complete `Content-Length`-prefixed JSON frame
/// arrives, or returns [`DapError::Disconnected`] on EOF before the
/// next frame starts.
pub fn read_frame<R: Read>(reader: &mut BufReader<R>) -> Result<Vec<u8>, DapError> {
    // Parse headers. The DAP spec only requires `Content-Length`,
    // but tolerates additional headers — we read until the blank
    // line, capturing the length when we see it.
    let mut content_length: Option<usize> = None;
    let mut header_line = String::new();
    loop {
        header_line.clear();
        let n = reader.read_line(&mut header_line)?;
        if n == 0 {
            return Err(DapError::Disconnected);
        }
        let trimmed = header_line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = value
                .trim()
                .parse::<usize>()
                .map(Some)
                .map_err(|_| DapError::BadHeader(trimmed.to_string()))?;
        }
        // Silently ignore other headers — `Content-Type` is
        // sometimes sent and DAP doesn't act on it.
    }
    let len =
        content_length.ok_or_else(|| DapError::BadHeader("missing Content-Length".to_string()))?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(body)
}

/// Write one DAP frame to a writer (typically stdout). Adds the
/// `Content-Length` header and the blank-line separator.
pub fn write_frame<W: Write>(writer: &mut W, body: &[u8]) -> Result<(), DapError> {
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(body)?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_one_frame() {
        let mut out: Vec<u8> = Vec::new();
        write_frame(&mut out, br#"{"hello":"world"}"#).unwrap();
        let mut reader = BufReader::new(Cursor::new(out));
        let frame = read_frame(&mut reader).unwrap();
        assert_eq!(frame, br#"{"hello":"world"}"#);
    }

    #[test]
    fn read_eof_is_disconnected() {
        let mut reader = BufReader::new(Cursor::new(Vec::<u8>::new()));
        let err = read_frame(&mut reader).unwrap_err();
        assert!(matches!(err, DapError::Disconnected));
    }

    #[test]
    fn extra_headers_are_ignored() {
        // Some DAP clients send `Content-Type`; make sure we don't
        // reject the frame on its presence.
        let frame = b"Content-Length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}";
        let mut reader = BufReader::new(Cursor::new(frame.to_vec()));
        let body = read_frame(&mut reader).unwrap();
        assert_eq!(body, b"{}");
    }

    #[test]
    fn missing_content_length_errors() {
        let frame = b"X-Other: foo\r\n\r\n{}";
        let mut reader = BufReader::new(Cursor::new(frame.to_vec()));
        let err = read_frame(&mut reader).unwrap_err();
        assert!(matches!(err, DapError::BadHeader(_)));
    }
}
