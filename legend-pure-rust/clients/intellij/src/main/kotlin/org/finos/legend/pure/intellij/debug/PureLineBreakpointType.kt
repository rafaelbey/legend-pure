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

import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.xdebugger.breakpoints.XBreakpointProperties
import com.intellij.xdebugger.breakpoints.XLineBreakpointType
import org.finos.legend.pure.intellij.PureFileType

/**
 * Line-breakpoint type for `.pure` source.
 *
 * IntelliJ's XDebugger framework requires a registered
 * `XLineBreakpointType` before its gutter accepts breakpoint
 * toggles for a language. Without this, clicking in the gutter
 * does nothing; with it, the user gets a red dot + the standard
 * "Mute / Disable / Properties" context menu.
 *
 * The runtime side decides whether the breakpoint actually fires —
 * `DapHooks::should_pause` (`crates/dap/src/hooks.rs`) checks the
 * canonical source path + 1-based line. So this type accepts every
 * `.pure` file, every line, and lets the runtime filter.
 *
 * Properties type is `EmptyProperties` (a no-op) — the MVP doesn't
 * carry condition/hit-count/log-message state. Those add
 * `XBreakpointProperties` subclasses when they ship.
 */
class PureLineBreakpointType : XLineBreakpointType<PureLineBreakpointType.EmptyProperties>(ID, TITLE) {
    override fun createBreakpointProperties(
        file: VirtualFile,
        line: Int,
    ): EmptyProperties? = null

    override fun canPutAt(
        file: VirtualFile,
        line: Int,
        project: Project,
    ): Boolean = file.fileType is PureFileType

    /**
     * Trivial properties holder. `XBreakpointProperties<T>` is
     * `PersistentStateComponent<T>` — we model T as `EmptyProperties`
     * and provide a no-op state methods, matching the
     * "no extra data" intent.
     */
    class EmptyProperties : XBreakpointProperties<EmptyProperties>() {
        override fun getState(): EmptyProperties = this
        override fun loadState(state: EmptyProperties) {}
    }

    companion object {
        const val ID = "legend-pure-line"
        const val TITLE = "Legend Pure Line Breakpoints"
    }
}
