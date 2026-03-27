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

//! # Legend Pure Parser — Protocol
//!
//! Defines the Protocol v1 JSON model and provides bidirectional conversion
//! between the parser AST and the protocol model. The protocol model mirrors
//! the Java `org.finos.legend.engine.protocol.pure.m3` package to produce
//! JSON byte-compatible with the existing Java/ANTLR4 parser.
//!
//! This is the only crate that depends on `serde`/`serde_json`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
