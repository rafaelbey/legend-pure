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

//! Error type for the DAP server.

use thiserror::Error;

/// Fatal server-side errors. Recoverable per-request errors are
/// reported back to the client as `Response.success = false` instead
/// of bubbling up through this type.
#[derive(Debug, Error)]
pub enum DapError {
    /// I/O failure on the stdio transport.
    #[error("I/O error on DAP transport: {0}")]
    Io(#[from] std::io::Error),
    /// Malformed JSON-RPC frame from the client.
    #[error("JSON parse error: {0}")]
    Parse(#[from] serde_json::Error),
    /// Header was missing `Content-Length` or otherwise unreadable.
    #[error("malformed header: {0}")]
    BadHeader(String),
    /// Client closed the connection (EOF on stdin). Not an error per
    /// se — the server uses it to break the message loop cleanly.
    #[error("client disconnected")]
    Disconnected,
}
