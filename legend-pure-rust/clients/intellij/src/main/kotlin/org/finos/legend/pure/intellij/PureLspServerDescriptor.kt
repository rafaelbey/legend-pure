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
package org.finos.legend.pure.intellij

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.diagnostic.logger
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.ProjectWideLspServerDescriptor
import com.intellij.platform.lsp.api.customization.LspCustomization
import org.finos.legend.pure.intellij.settings.LegendPureSettings
import java.nio.charset.StandardCharsets

/**
 * Describes how to spawn the `legend lsp` server for a project.
 *
 * Project-wide (single process per project) because the server's
 * `Workspace` model already shadows open buffers against the
 * filesystem-resolved repos — there is no benefit to per-file
 * descriptors and a real cost (re-running the classpath cascade per
 * file).
 */
class PureLspServerDescriptor(project: Project) :
    ProjectWideLspServerDescriptor(project, "Legend Pure") {

    init {
        // One-shot startup banner so the user can verify the customised
        // descriptor is actually being constructed when a `.pure` file
        // opens. Look for it in *Help → Show Log in Finder*. If you
        // see this line, ⌘-hover navigation should be wired.
        LOG.info(
            "PureLspServerDescriptor created — goto-def + hover + completion enabled via " +
                "LspCustomization() (replaces the default DeprecatedLspCustomization)",
        )
    }

    /**
     * Opt the descriptor in to LSP-driven features.
     *
     * IntelliJ's LSP integration treats every feature as opt-in. By
     * default the descriptor's `lspCustomization` is a
     * `DeprecatedLspCustomization` which keeps all booleans off
     * regardless of what the server advertises in `initialize`.
     * Without this override, ⌘-hover never underlines navigable
     * identifiers and ⌘-click does nothing — the platform never
     * dispatches `textDocument/definition` to our server.
     *
     * The no-arg `LspCustomization()` constructor instantiates a
     * `*Support` instance for every slot (goto-definition, hover,
     * completion, find-references, …). Slots whose server-side
     * handler is not yet implemented (find-references, formatting,
     * code-actions) just no-op against the server: the platform
     * sends the request, the server returns nothing, the IDE shows
     * an empty result. No inert UI to worry about for the MVP.
     *
     * `get()` is used (not `=`) so the override survives super-class
     * constructor ordering — the parent runs through its init before
     * our subclass field would be assigned, but the property getter
     * computes on every access and always returns the right value.
     */
    override val lspCustomization: LspCustomization
        get() = INSTANCE

    override fun isSupportedFile(file: VirtualFile): Boolean =
        file.fileType is PureFileType

    override fun createCommandLine(): GeneralCommandLine {
        val settings = LegendPureSettings.getInstance(project)
        val exe = settings.resolveExecutableOrThrow()
        val cmd = GeneralCommandLine(exe.toString())
            .withCharset(StandardCharsets.UTF_8)
            .apply {
                addParameter("lsp")
                val classpath = settings.classpathTomlPath.trim()
                if (classpath.isNotEmpty()) {
                    addParameters("--classpath", classpath)
                }
                project.basePath?.let { withWorkDirectory(it) }
            }
        LOG.info("spawning legend LSP: ${cmd.commandLineString}")
        return cmd
    }

    companion object {
        private val LOG = logger<PureLspServerDescriptor>()
        // One-time customization shared by all descriptor instances.
        // Constructing in a static init avoids any subclass-vs-parent
        // constructor-ordering surprise where the override might be
        // queried before the field is initialized.
        private val INSTANCE: LspCustomization = LspCustomization()
    }
}
