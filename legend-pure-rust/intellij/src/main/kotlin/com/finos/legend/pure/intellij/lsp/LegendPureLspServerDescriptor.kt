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
 * Resolution rules:
 *   - **Binary**: `LEGEND_PURE_BIN` env var, then `legend` on PATH.
 *   - **Classpath**: walk up from the project base for the first
 *     `legend-pure-classpath.toml` and pass it via `--classpath`.
 *     Trusting the LSP's own ancestor-walk discovery is unreliable
 *     because the LSP's cwd is the *project* root, not necessarily
 *     the directory containing the descriptor — and silently falling
 *     through to the embedded-only fallback gives the user no
 *     feedback for files in their own repo.
 *
 * Settings-panel overrides are a follow-up.
 */
class LegendPureLspServerDescriptor(project: Project) : OSProcessStreamConnectionProvider() {

    init {
        val baseDir = project.basePath?.let { java.nio.file.Paths.get(it) }
        val classpathToml = baseDir?.let(::discoverClasspathToml)

        val params = mutableListOf("lsp")
        if (classpathToml != null) {
            params += "--classpath"
            params += classpathToml.toString()
        }

        val command = GeneralCommandLine().apply {
            withExePath(System.getenv("LEGEND_PURE_BIN") ?: "legend")
            withParameters(params)
            baseDir?.let { withWorkDirectory(it.toString()) }
        }
        super.setCommandLine(command)
    }

    private companion object {
        private const val DESCRIPTOR_NAME = "legend-pure-classpath.toml"

        /**
         * Walk `start` and its ancestors looking for the first
         * `legend-pure-classpath.toml`. Returns null if none is found
         * within the file-system root.
         */
        fun discoverClasspathToml(start: java.nio.file.Path): java.nio.file.Path? {
            var dir: java.nio.file.Path? = start
            while (dir != null) {
                val candidate = dir.resolve(DESCRIPTOR_NAME)
                if (java.nio.file.Files.isRegularFile(candidate)) {
                    return candidate
                }
                dir = dir.parent
            }
            return null
        }
    }
}
