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
package org.finos.legend.pure.intellij.util

import com.intellij.openapi.util.SystemInfo
import java.io.File
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths

/**
 * Resolves the path to the `legend` CLI executable.
 *
 * Resolution order:
 * 1. If [explicitPath] is non-blank, return it verbatim. The caller is
 *    expected to validate file existence + executability separately so
 *    Settings UI can give a clear error.
 * 2. Otherwise look up `legend` (or `legend.exe` on Windows) by walking
 *    the OS `PATH` environment variable.
 *
 * Returns `null` when no candidate is found — callers should surface
 * this as a user-visible error rather than spawning a doomed process.
 */
object ExecutableResolver {

    private const val DEFAULT_NAME = "legend"

    fun resolve(explicitPath: String?): Path? {
        if (!explicitPath.isNullOrBlank()) {
            val explicit = Paths.get(explicitPath.trim())
            return if (Files.exists(explicit)) explicit else null
        }
        return resolveFromPath(DEFAULT_NAME)
    }

    /**
     * Walk `$PATH` looking for the named binary. Honours `.exe`/`.bat`/`.cmd`
     * extensions on Windows. Visibility is package-private so unit tests can
     * exercise it directly.
     */
    fun resolveFromPath(name: String): Path? {
        val pathEnv = System.getenv("PATH") ?: return null
        val candidates = if (SystemInfo.isWindows) {
            listOf("$name.exe", "$name.bat", "$name.cmd", name)
        } else {
            listOf(name)
        }
        for (segment in pathEnv.split(File.pathSeparator)) {
            if (segment.isBlank()) continue
            val dir = try {
                Paths.get(segment)
            } catch (_: Exception) {
                continue
            }
            for (candidate in candidates) {
                val full = dir.resolve(candidate)
                if (Files.isRegularFile(full) && Files.isExecutable(full)) {
                    return full
                }
            }
        }
        return null
    }
}
