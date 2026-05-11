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
package org.finos.legend.pure.intellij.settings

import com.intellij.openapi.components.PersistentStateComponent
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.service
import com.intellij.openapi.project.Project
import com.intellij.util.xmlb.XmlSerializerUtil
import org.finos.legend.pure.intellij.util.ExecutableResolver
import java.nio.file.Files
import java.nio.file.Path

/**
 * Project-level persistent state for the Legend Pure plugin.
 *
 * Backed by `.idea/legend-pure.xml` per project. Two strings, both
 * optional:
 *
 *  - [legendExecutablePath] — explicit path to the `legend` CLI binary.
 *    When blank, the plugin resolves `legend` via the OS `$PATH`.
 *  - [classpathTomlPath] — explicit path to a
 *    `legend-pure-classpath.toml`. When blank, no `--classpath` flag is
 *    passed and the server runs its built-in cascade
 *    (env var → ancestor walk → next-to-binary → embedded platform).
 */
@State(
    name = "LegendPureSettings",
    storages = [Storage("legend-pure.xml")],
)
@Service(Service.Level.PROJECT)
class LegendPureSettings : PersistentStateComponent<LegendPureSettings.State> {

    /** Mutable persisted bag — IntelliJ requires public mutable fields. */
    class State {
        var legendExecutablePath: String = ""
        var classpathTomlPath: String = ""
    }

    private var state: State = State()

    override fun getState(): State = state
    override fun loadState(other: State) {
        XmlSerializerUtil.copyBean(other, state)
    }

    var legendExecutablePath: String
        get() = state.legendExecutablePath
        set(value) {
            state.legendExecutablePath = value
        }

    var classpathTomlPath: String
        get() = state.classpathTomlPath
        set(value) {
            state.classpathTomlPath = value
        }

    /**
     * Resolve the executable path or throw a [ConfigurationException] with
     * a user-friendly message. Used by the LSP descriptor at server-spawn
     * time — bad config should fail loudly, not produce a phantom
     * "server crashed" notification.
     */
    @Throws(ConfigurationException::class)
    fun resolveExecutableOrThrow(): Path {
        val resolved = ExecutableResolver.resolve(legendExecutablePath.takeIf { it.isNotBlank() })
            ?: throw ConfigurationException(
                if (legendExecutablePath.isBlank())
                    "`legend` not found on PATH. Set the path explicitly under " +
                        "Settings → Languages & Frameworks → Legend Pure."
                else
                    "Configured `legend` executable does not exist: $legendExecutablePath",
            )
        if (!Files.isExecutable(resolved)) {
            throw ConfigurationException("File is not executable: $resolved")
        }
        return resolved
    }

    /** Validation result for Settings UI; no I/O side-effects. */
    fun validate(): ValidationResult {
        val problems = mutableListOf<String>()
        val exePath = legendExecutablePath.trim()
        if (exePath.isNotEmpty()) {
            val p = runCatching { Path.of(exePath) }.getOrNull()
            when {
                p == null -> problems += "Executable path is not a valid filesystem path."
                !Files.exists(p) -> problems += "Executable path does not exist: $exePath"
                !Files.isExecutable(p) -> problems += "Executable path is not executable: $exePath"
            }
        }
        val cpPath = classpathTomlPath.trim()
        if (cpPath.isNotEmpty()) {
            val p = runCatching { Path.of(cpPath) }.getOrNull()
            when {
                p == null -> problems += "Classpath TOML path is not a valid filesystem path."
                !Files.exists(p) -> problems += "Classpath TOML does not exist: $cpPath"
                !Files.isReadable(p) -> problems += "Classpath TOML is not readable: $cpPath"
            }
        }
        return if (problems.isEmpty()) ValidationResult.Ok else ValidationResult.Errors(problems)
    }

    sealed class ValidationResult {
        data object Ok : ValidationResult()
        data class Errors(val messages: List<String>) : ValidationResult()
    }

    /**
     * Thrown by [resolveExecutableOrThrow]. Distinct from the IntelliJ
     * platform's own ConfigurationException so we don't accidentally
     * leak settings-dialog wiring into the LSP descriptor.
     */
    class ConfigurationException(message: String) : RuntimeException(message)

    companion object {
        fun getInstance(project: Project): LegendPureSettings = project.service()
    }
}
