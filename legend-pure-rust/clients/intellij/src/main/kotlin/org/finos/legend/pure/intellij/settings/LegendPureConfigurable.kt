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

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.process.CapturingProcessHandler
import com.intellij.openapi.fileChooser.FileChooserDescriptor
import com.intellij.openapi.fileChooser.FileChooserDescriptorFactory
import com.intellij.openapi.options.Configurable
import com.intellij.openapi.options.ConfigurationException as PlatformConfigurationException
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.Messages
import com.intellij.openapi.ui.TextFieldWithBrowseButton
import com.intellij.ui.components.JBLabel
import com.intellij.util.ui.FormBuilder
import org.finos.legend.pure.intellij.util.ExecutableResolver
import java.nio.charset.StandardCharsets
import javax.swing.JButton
import javax.swing.JComponent
import javax.swing.JPanel

/**
 * Settings UI under
 * *Settings → Languages & Frameworks → Legend Pure*.
 *
 * Two text-fields (executable path, classpath TOML) plus a "Test
 * connection" button that runs `<legend> --version` and shows the
 * output. The button is the cheapest possible end-to-end check — it
 * exercises the full PATH-resolution + spawn pipeline without
 * requiring a project to be open.
 */
class LegendPureConfigurable(private val project: Project) : Configurable {

    private val settings = LegendPureSettings.getInstance(project)

    private val executableField = TextFieldWithBrowseButton().apply {
        // Single regular file (not a folder, not a jar). Hand-rolled
        // because there's no FileChooserDescriptorFactory factory for an
        // arbitrary single file with no extension hint.
        val descriptor = FileChooserDescriptor(true, false, false, false, false, false)
            .withTitle("Legend Executable")
            .withDescription("Path to the `legend` CLI binary. Leave blank to resolve via OS PATH.")
        addBrowseFolderListener(project, descriptor)
    }

    private val classpathField = TextFieldWithBrowseButton().apply {
        val descriptor = FileChooserDescriptorFactory.createSingleFileDescriptor("toml")
            .withTitle("Legend Pure Classpath")
            .withDescription("Path to legend-pure-classpath.toml. Leave blank to auto-discover.")
        addBrowseFolderListener(project, descriptor)
    }

    private val testButton = JButton("Test Connection").apply {
        addActionListener { runVersionCheck() }
    }

    private var panel: JPanel? = null

    override fun getDisplayName(): String = "Legend Pure"

    override fun createComponent(): JComponent {
        val helpExecutable = JBLabel(
            "<html>Path to the <code>legend</code> CLI executable. " +
                "Leave blank to resolve via the OS <code>PATH</code>.</html>",
        )
        val helpClasspath = JBLabel(
            "<html>Optional path to a <code>legend-pure-classpath.toml</code>. " +
                "Leave blank to let the server auto-discover the classpath " +
                "(same cascade as <code>legend check</code> / <code>legend compile</code>).</html>",
        )
        val built = FormBuilder.createFormBuilder()
            .addLabeledComponent(JBLabel("Legend executable:"), executableField, 1, false)
            .addComponentToRightColumn(helpExecutable, 1)
            .addLabeledComponent(JBLabel("Classpath TOML:"), classpathField, 1, false)
            .addComponentToRightColumn(helpClasspath, 1)
            .addComponent(testButton, 1)
            .addComponentFillVertically(JPanel(), 0)
            .panel
        panel = built
        reset()
        return built
    }

    override fun isModified(): Boolean {
        return executableField.text.trim() != settings.legendExecutablePath ||
            classpathField.text.trim() != settings.classpathTomlPath
    }

    @Throws(PlatformConfigurationException::class)
    override fun apply() {
        val pending = LegendPureSettings.State().apply {
            legendExecutablePath = executableField.text.trim()
            classpathTomlPath = classpathField.text.trim()
        }
        // Snapshot, validate, commit-or-roll-back so a bad value never
        // ends up persisted.
        val previousExe = settings.legendExecutablePath
        val previousCp = settings.classpathTomlPath
        settings.legendExecutablePath = pending.legendExecutablePath
        settings.classpathTomlPath = pending.classpathTomlPath
        when (val result = settings.validate()) {
            is LegendPureSettings.ValidationResult.Ok -> Unit
            is LegendPureSettings.ValidationResult.Errors -> {
                settings.legendExecutablePath = previousExe
                settings.classpathTomlPath = previousCp
                throw PlatformConfigurationException(result.messages.joinToString("\n"))
            }
        }
    }

    override fun reset() {
        executableField.text = settings.legendExecutablePath
        classpathField.text = settings.classpathTomlPath
    }

    private fun runVersionCheck() {
        val explicit = executableField.text.trim().takeIf { it.isNotBlank() }
        val exe = ExecutableResolver.resolve(explicit)
        if (exe == null) {
            Messages.showErrorDialog(
                panel,
                if (explicit == null) "`legend` not found on PATH."
                else "Executable not found: $explicit",
                "Legend Pure",
            )
            return
        }
        val cmd = GeneralCommandLine(exe.toString(), "--version").apply {
            charset = StandardCharsets.UTF_8
        }
        val output = try {
            CapturingProcessHandler(cmd).runProcess(5_000)
        } catch (e: Exception) {
            Messages.showErrorDialog(
                panel,
                "Failed to spawn `${cmd.commandLineString}`: ${e.message}",
                "Legend Pure",
            )
            return
        }
        if (output.exitCode == 0) {
            Messages.showInfoMessage(panel, output.stdout.trim(), "Legend Pure")
        } else {
            Messages.showErrorDialog(
                panel,
                "Exit ${output.exitCode}\n${output.stderr.trim().ifEmpty { output.stdout.trim() }}",
                "Legend Pure",
            )
        }
    }
}
