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

//! Native functions over `Relation` / `RelationType` / `Column` /
//! `ColSpec` / `ColSpecArray` / `TDS`.
//!
//! Mirrors `legend-pure-runtime/.../natives/essentials/meta/type/relation`.
//! All M3 identification is by ElementId (`m3_paths::resolve`), never
//! classifier-string matching — see `feedback_no_classifier_string_compare`.
//!
//! Each native lives in its own file under this directory so multiple
//! contributors (and parallel work-streams) can land additions without
//! the merge-conflict surface of a single 2000-line module. Internal
//! helpers — heap walks, TDS row-access, etc. — are in `shared.rs`.

mod add_columns;
mod concatenate;
mod distinct;
mod drop;
mod filter;
mod limit;
mod shared;
mod size;
mod string_to_tds;

use crate::native::NativeRegistry;

pub use add_columns::AddColumns;
pub use concatenate::Concatenate;
pub use distinct::Distinct;
pub use drop::Drop;
pub use filter::Filter;
pub use limit::Limit;
pub use size::Size;
pub use string_to_tds::StringToTDS;

/// Register relation native functions into the registry under their
/// mangled Pure FQNs.
pub fn register(registry: &mut NativeRegistry) {
    registry.register(
        "addColumns_RelationType_1__ColSpecArray_1__RelationType_1_",
        AddColumns,
    );
    registry.register("stringToTDS_String_1__TDS_1_", StringToTDS);
    registry.register("size_Relation_1__Integer_1_", Size);
    registry.register("distinct_Relation_1__Relation_1_", Distinct);
    registry.register(
        "concatenate_Relation_1__Relation_1__Relation_1_",
        Concatenate,
    );
    registry.register("filter_Relation_1__Function_1__Relation_1_", Filter);
    registry.register("limit_Relation_1__Integer_1__Relation_1_", Limit);
    registry.register("drop_Relation_1__Integer_1__Relation_1_", Drop);
}
