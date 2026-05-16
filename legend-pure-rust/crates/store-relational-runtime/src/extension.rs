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

//! [`RelationalStoreExtension`] — the plug-in entry point that registers
//! every relational-store native into a
//! [`legend_pure_runtime::native::NativeRegistry`].
//!
//! The mangled keys are copied verbatim from Java's
//! `RelationalExtensionInterpreted` (the upstream interpreted runtime's
//! extension table) so both stacks dispatch the same Pure calls to the
//! same native implementations.

use legend_pure_runtime::native::{NativeRegistry, RUNTIME_EXTENSIONS, RuntimeExtension};
use linkme::distributed_slice;

use crate::natives::execute_in_db::ExecuteInDb;
use crate::natives::fetch_metadata::{
    FetchDbColumnsMetaData, FetchDbImportedKeysMetaData, FetchDbPrimaryKeysMetaData,
    FetchDbSchemasMetaData, FetchDbTablesMetaData,
};
use crate::natives::load_csv::LoadCsvToDbTable;
use crate::natives::load_values::LoadValuesToDbTable;
use crate::natives::temp_table::{CreateTempTable, CreateTempTableWithFinally, DropTempTable};

/// Registers the 10 relational-store native bodies into a runtime native
/// registry.
///
/// Pass an instance to
/// [`legend_pure_runtime::eval::Evaluator::new_default_with_extensions`]:
///
/// ```ignore
/// use legend_pure_store_relational_runtime::RelationalStoreExtension;
/// let eval = Evaluator::new_default_with_extensions(&model, &[&RelationalStoreExtension]);
/// ```
#[derive(Debug, Default)]
pub struct RelationalStoreExtension;

/// Self-registration into the runtime's `RUNTIME_EXTENSIONS`
/// distributed slice so any binary that depends on this crate picks
/// up the relational natives via `NativeRegistry::discovered()` with
/// no per-binary wiring.
#[distributed_slice(RUNTIME_EXTENSIONS)]
static RELATIONAL_STORE_EXTENSION: &(dyn RuntimeExtension + Sync) = &RelationalStoreExtension;

impl RuntimeExtension for RelationalStoreExtension {
    fn name(&self) -> &'static str {
        "legend-pure-store-relational-runtime"
    }

    fn register_natives(&self, r: &mut NativeRegistry) {
        // Mangled keys copied from Java's RelationalExtensionInterpreted
        // (legend-pure-runtime-java-extension-interpreted-store-relational
        //  /.../RelationalExtensionInterpreted.java:33-57). Mirroring them
        // verbatim is what guarantees the Pure → native dispatch agrees
        // across stacks; never edit one side without the other.
        r.register(
            "executeInDb_String_1__DatabaseConnection_1__Integer_1__Integer_1__ResultSet_1_",
            ExecuteInDb,
        );

        r.register(
            "loadCsvToDbTable_String_1__Table_1__DatabaseConnection_1__Integer_$0_1$__Nil_0_",
            LoadCsvToDbTable,
        );

        // Two overloads, same implementation:
        // 1. `tableData: List<List<Any>>[*]`
        // 2. `tableData: List<List<Any>>[1]`
        r.register(
            "loadValuesToDbTable_List_MANY__Table_1__DatabaseConnection_1__Nil_0_",
            LoadValuesToDbTable,
        );
        r.register(
            "loadValuesToDbTable_List_1__Table_1__DatabaseConnection_1__Nil_0_",
            LoadValuesToDbTable,
        );

        r.register(
            "fetchDbTablesMetaData_DatabaseConnection_1__String_$0_1$__String_$0_1$__ResultSet_1_",
            FetchDbTablesMetaData,
        );
        r.register(
            "fetchDbColumnsMetaData_DatabaseConnection_1__String_$0_1$__String_$0_1$__String_$0_1$__ResultSet_1_",
            FetchDbColumnsMetaData,
        );
        r.register(
            "fetchDbSchemasMetaData_DatabaseConnection_1__String_$0_1$__ResultSet_1_",
            FetchDbSchemasMetaData,
        );
        r.register(
            "fetchDbPrimaryKeysMetaData_DatabaseConnection_1__String_$0_1$__String_1__ResultSet_1_",
            FetchDbPrimaryKeysMetaData,
        );
        r.register(
            "fetchDbImportedKeysMetaData_DatabaseConnection_1__String_$0_1$__String_1__ResultSet_1_",
            FetchDbImportedKeysMetaData,
        );

        // Two overloads of `createTempTable`. Each gets its own zero-sized
        // type so dispatch never relies on positional arg-shape probing
        // (memory: feedback_split_natives_by_shape) — they share their
        // body via `do_create_temp_table`.
        r.register(
            "createTempTable_String_1__Column_MANY__Function_1__DatabaseConnection_1__Nil_0_",
            CreateTempTable,
        );
        r.register(
            "createTempTable_String_1__Column_MANY__Function_1__Boolean_1__DatabaseConnection_1__Nil_0_",
            CreateTempTableWithFinally,
        );

        r.register(
            "dropTempTable_String_1__DatabaseConnection_1__Nil_0_",
            DropTempTable,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_all_ten_native_keys() {
        let mut registry = NativeRegistry::new();
        RelationalStoreExtension.register_natives(&mut registry);

        // Each FQN must be addressable post-registration.
        for key in [
            "executeInDb_String_1__DatabaseConnection_1__Integer_1__Integer_1__ResultSet_1_",
            "loadCsvToDbTable_String_1__Table_1__DatabaseConnection_1__Integer_$0_1$__Nil_0_",
            "loadValuesToDbTable_List_MANY__Table_1__DatabaseConnection_1__Nil_0_",
            "loadValuesToDbTable_List_1__Table_1__DatabaseConnection_1__Nil_0_",
            "fetchDbTablesMetaData_DatabaseConnection_1__String_$0_1$__String_$0_1$__ResultSet_1_",
            "fetchDbColumnsMetaData_DatabaseConnection_1__String_$0_1$__String_$0_1$__String_$0_1$__ResultSet_1_",
            "fetchDbSchemasMetaData_DatabaseConnection_1__String_$0_1$__ResultSet_1_",
            "fetchDbPrimaryKeysMetaData_DatabaseConnection_1__String_$0_1$__String_1__ResultSet_1_",
            "fetchDbImportedKeysMetaData_DatabaseConnection_1__String_$0_1$__String_1__ResultSet_1_",
            "createTempTable_String_1__Column_MANY__Function_1__DatabaseConnection_1__Nil_0_",
            "createTempTable_String_1__Column_MANY__Function_1__Boolean_1__DatabaseConnection_1__Nil_0_",
            "dropTempTable_String_1__DatabaseConnection_1__Nil_0_",
        ] {
            assert!(
                registry.get(key).is_some(),
                "expected mangled key `{key}` to be registered"
            );
        }
        // 10 distinct natives, 12 registrations (2 createTempTable + 2 loadValuesToDbTable overloads).
        assert_eq!(registry.len(), 12);
    }
}
