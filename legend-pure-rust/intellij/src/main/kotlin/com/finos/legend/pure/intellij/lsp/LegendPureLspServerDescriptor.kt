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

package com.finos.legend.pure.intellij.lsp

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.project.Project
import com.redhat.devtools.lsp4ij.server.OSProcessStreamConnectionProvider

/**
 * Builds the OS process command line for `legend lsp`.
 *
 * Resolution rule for the `legend` binary:
 *   1. The `LEGEND_PURE_BIN` environment variable.
 *   2. `legend` on `PATH`.
 *
 * No `--classpath` flag is forwarded yet — the LSP's built-in
 * discovery cascade (env → ancestor walk → next-to-binary) handles
 * the typical case. Settings panel + explicit overrides are
 * follow-ups (see plan §5c).
 */
class LegendPureLspServerDescriptor(project: Project) : OSProcessStreamConnectionProvider() {

    init {
        val command = GeneralCommandLine().apply {
            withExePath(System.getenv("LEGEND_PURE_BIN") ?: "legend")
            withParameters("lsp")
            // Run from the project root so the LSP's ancestor-walk
            // descriptor discovery finds `legend-pure-classpath.toml`.
            project.basePath?.let { withWorkDirectory(it) }
        }
        super.setCommandLine(command)
    }
}
