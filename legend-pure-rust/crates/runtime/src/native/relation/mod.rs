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

//! Platform-defined native functions over `RelationType` / `Column` /
//! `ColSpec` / `ColSpecArray` / `TDS`.
//!
//! Only natives whose *declarations* live in the legend-pure platform
//! repos (`platform`, `platform_dsl_tds`) belong here:
//!
//! - `addColumns(RelationType, ColSpecArray)` — declared in
//!   `platform/.../essential/meta/type/relation/addColumns.pure`
//! - `stringToTDS(String):TDS` — declared in `platform_dsl_tds/tds.pure`
//! - `tdsToCsv(TDS):String` — declared in `platform_dsl_tds/tds.pure`;
//!   backs the derived `TDS.csv()` qualified property
//!
//! Engine-side relation natives (`filter`/`sort`/`distinct`/`extend`/
//! `select`/`rename`/`columns`/`limit`/`drop`/`concatenate`/`size`/
//! `ascending`/`descending`) — whose declarations live in
//! `core_functions_relation` — ship as a runtime extension in the
//! legend-engine workspace
//! (`legend-engine-rust/crates/natives-functions-relation`).
//!
//! Shared row-tuple, CSV, and heap-walk helpers stay in this crate's
//! `shared` module (publicly exposed) so the engine extension can
//! reuse them.
//!
//! All M3 identification is by ElementId (`m3_paths::resolve`), never
//! classifier-string matching — see `feedback_no_classifier_string_compare`.

mod add_columns;
pub mod shared;
mod string_to_tds;
mod tds_to_csv;

use crate::native::NativeRegistry;

pub use add_columns::AddColumns;
pub use string_to_tds::StringToTDS;
pub use tds_to_csv::TdsToCsv;

/// Register the platform-defined relation natives into the registry.
///
/// Called from [`NativeRegistry::standard`] — these natives are
/// part of the minimum surface every Pure program can call. Engine-
/// defined relation natives (`filter`, `sort`, `extend`, …) register
/// separately through the
/// [`RuntimeExtension`](crate::native::RuntimeExtension) SPI from the
/// `legend-engine-rust-natives-functions-relation` crate.
pub fn register(registry: &mut NativeRegistry) {
    registry.register(
        "addColumns_RelationType_1__ColSpecArray_1__RelationType_1_",
        AddColumns,
    );
    registry.register("stringToTDS_String_1__TDS_1_", StringToTDS);
    registry.register("tdsToCsv_TDS_1__String_1_", TdsToCsv);
}
