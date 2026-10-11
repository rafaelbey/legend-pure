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

//! Date and time natives over [`crate::date`]: the constructors (`date`,
//! `parseDate`), the arithmetic (`adjust`, `dateDiff`, `datePart`), the
//! component accessors (`year`, `monthNumber`, `dayOfMonth`, `hour`,
//! `minute`, `second`) and the precision predicates (`hasDay`, `hasHour`,
//! `hasSubsecond`, …).
//!
//! Each native is fished from the archive one `impl NativeFunction` at a
//! time, registered under Java's mangled key, when a PCT row asks for it.

use crate::native::NativeRegistry;

/// Register the fished date and time natives.
pub fn register(_registry: &mut NativeRegistry) {}
