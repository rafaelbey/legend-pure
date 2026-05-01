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

//! `IslandProtocol` for the TDS inline island.
//!
//! Tag: `"TDS"`. Converts [`TDSExpr`] AST into the v1 protocol's
//! `ClassInstance { _type: "TDS", … }` value specification:
//!
//! ```json
//! {
//!     "_type": "TDS",
//!     "columns": [
//!         { "name": "value", "type": "Float", "multiplicity": "1" },
//!         { "name": "name",  "type": null,    "multiplicity": null }
//!     ],
//!     "rows": [["1", "A"], ["2", "B"]],
//!     "sourceInformation": { … }
//! }
//! ```
//!
//! The metaclass at compile time is
//! `meta::pure::metamodel::relation::TDS<T>` (see
//! [`crate::compiler`], commit #14). For now the protocol shape
//! preserves the parsed structure verbatim; compiler extension turns
//! it into a `csv: String[1]` instance later.
//!
//! Plug-in callers register via [`default_island_protocols`] when
//! calling [`legend_pure_parser_protocol::dispatch_island_convert`].

use legend_pure_parser_ast::island::IslandContent;
use legend_pure_parser_protocol::IslandProtocol;
use legend_pure_parser_protocol::v1::value_spec::{ClassInstance, ValueSpecification};

use crate::ast::{TAG, TDSColumn, TDSExpr};

/// Protocol converter for TDS islands. Emits the canonical
/// `_type: "TDS"` JSON shape.
pub struct TDSIslandProtocol;

impl IslandProtocol for TDSIslandProtocol {
    fn tag(&self) -> &str {
        TAG
    }

    fn convert(
        &self,
        content: &dyn IslandContent,
    ) -> legend_pure_parser_protocol::island_protocol::Result<ValueSpecification> {
        let tds = content.as_any().downcast_ref::<TDSExpr>().ok_or_else(|| {
            use serde::ser::Error as _;
            serde_json::Error::custom(format!(
                "TDSIslandProtocol: content with tag '{}' is not a TDSExpr",
                content.tag()
            ))
        })?;
        Ok(convert_tds(tds))
    }
}

/// Convenience helper — `vec![Box::new(TDSIslandProtocol)]` for
/// callers that want the TDS DSL protocol converter registered.
#[must_use]
pub fn default_island_protocols() -> Vec<Box<dyn IslandProtocol>> {
    vec![Box::new(TDSIslandProtocol)]
}

fn convert_tds(tds: &TDSExpr) -> ValueSpecification {
    let columns: Vec<serde_json::Value> = tds.columns.iter().map(convert_column).collect();
    let rows: Vec<serde_json::Value> = tds
        .rows
        .iter()
        .map(|row| {
            let cells: Vec<serde_json::Value> = row
                .iter()
                .map(|c| serde_json::Value::String(c.raw.to_string()))
                .collect();
            serde_json::Value::Array(cells)
        })
        .collect();

    ValueSpecification::ClassInstance(ClassInstance {
        type_name: TAG.to_string(),
        value: serde_json::json!({
            "_type": TAG,
            "columns": columns,
            "rows": rows,
        }),
        source_information: Some(
            legend_pure_parser_protocol::v1::source_info::SourceInformation::from(&tds.source_info),
        ),
    })
}

fn convert_column(col: &TDSColumn) -> serde_json::Value {
    serde_json::json!({
        "name": col.name.to_string(),
        "type": col.type_ref.as_ref().map(|t| t.name.to_string()),
        "multiplicity": col
            .type_ref
            .as_ref()
            .and_then(|t| t.multiplicity.as_ref())
            .map(smol_str::SmolStr::to_string),
    })
}
