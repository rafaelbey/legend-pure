/*
 * Copyright 2026 Goldman Sachs
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
package org.finos.legend.pure.intellij.debug

import com.intellij.platform.dap.DebugAdapterId

/**
 * Singleton id for the Legend Pure DAP adapter.
 *
 * `type` is the machine-readable key (lower-case, kebab-style); the
 * IDE telemetry + DAP `initialize` request's `adapterID` field uses
 * it. `presentableName` shows up in the Run/Debug UI's adapter
 * picker.
 *
 * The class is an `object` because [`DebugAdapterId`] doesn't carry
 * per-instance state — the same id flows through every descriptor
 * the support provider hands out, and equality is by `type` string.
 */
object PureDebugAdapterId : DebugAdapterId(
    /* type = */ "legend-pure",
    /* presentableName = */ "Legend Pure",
)
