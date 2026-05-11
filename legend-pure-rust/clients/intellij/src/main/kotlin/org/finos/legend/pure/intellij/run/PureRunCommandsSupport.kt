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
package org.finos.legend.pure.intellij.run

import com.google.gson.Gson
import com.google.gson.JsonArray
import com.google.gson.JsonElement
import com.google.gson.JsonObject
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.diagnostic.logger
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.popup.JBPopupFactory
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.openapi.wm.WindowManager
import com.intellij.platform.lsp.api.LspServer
import com.intellij.platform.lsp.api.LspServerManager
import com.intellij.platform.lsp.api.customization.LspCommandsSupport
import com.intellij.ui.awt.RelativePoint
import org.eclipse.lsp4j.Command
import org.eclipse.lsp4j.ExecuteCommandParams
import org.finos.legend.pure.intellij.PureLspServerSupportProvider

/**
 * Routes `legend.run` and `legend.runTest` clicks back to the LSP
 * server via `workspace/executeCommand`.
 *
 * Why LSP and not a spawned CLI subprocess: the running `legend lsp`
 * already has the user's workspace compiled (open buffers + the
 * configured classpath cascade resolved into a `PureModel`). Asking
 * it to run a function reuses that exact compiled state — same
 * resolution rules the editor uses for diagnostics, completions and
 * goto-def. Shelling out to a fresh `legend run` would re-load only
 * the embedded platform and miss every user-defined function, which
 * is exactly the `Function not found: <fqn>` failure mode this
 * design replaces.
 *
 * The server returns `{ ok: bool, fqn: string, value?: string,
 * error?: string }`. We surface either branch as a
 * `Notification` (info on success, error otherwise).
 *
 * Same entry point services two paths:
 *  - LSP code-lens click → [`executeCommand`] (this class is the
 *    `LspCommandsCustomizer` installed on
 *    [`PureLspCustomization`]).
 *  - Gutter ▶ click → [`Companion.runViaLsp`] from
 *    `PureRunLineMarkerContributor`.
 */
class PureRunCommandsSupport : LspCommandsSupport() {
    override fun executeCommand(
        server: LspServer,
        contextFile: VirtualFile,
        command: Command,
    ) {
        when (command.command) {
            "legend.run", "legend.runTest" -> {
                val fqn = command.arguments?.firstOrNull()?.let { extractString(it) }
                if (fqn.isNullOrBlank()) {
                    notify(
                        server.project,
                        command.command,
                        "Run command missing function FQN argument",
                        NotificationType.ERROR,
                    )
                    return
                }
                runOnServer(server, command.command, listOf<Any?>(fqn), titleFor(command.command, fqn))
            }
            "legend.runPCT" -> {
                val fqn = command.arguments?.firstOrNull()?.let { extractString(it) }
                if (fqn.isNullOrBlank()) {
                    notify(
                        server.project,
                        command.command,
                        "PCT command missing function FQN argument",
                        NotificationType.ERROR,
                    )
                    return
                }
                runPctViaLsp(server.project, fqn)
            }
            else -> super.executeCommand(server, contextFile, command)
        }
    }

    private fun extractString(arg: Any): String? {
        // lsp4j parses `Command.arguments` as a list of either
        // `String` or `JsonElement` depending on which gson path
        // the message took. Tolerate both shapes.
        return when (arg) {
            is String -> arg
            is JsonElement ->
                if (arg.isJsonPrimitive && arg.asJsonPrimitive.isString) arg.asString
                else arg.toString()
            else -> arg.toString()
        }
    }

    companion object {
        private val LOG = logger<PureRunCommandsSupport>()

        /** Notification group id declared in `plugin.xml`. */
        private const val NOTIFICATION_GROUP = "Legend Pure"

        /** Per-call timeout for `workspace/executeCommand`. Long
         *  enough for a moderately heavy parameterless function but
         *  short enough that an accidental infinite loop in user
         *  code doesn't wedge the IDE. The user can interrupt by
         *  closing the project. */
        private const val EXECUTE_TIMEOUT_MS = 60_000

        /**
         * Public entry point used by [`PureRunLineMarkerContributor`]'s
         * action. Resolves the project's running LSP server and
         * dispatches `workspace/executeCommand`.
         *
         * Returns immediately; the call runs on a pooled thread and
         * surfaces the result as a [Notification].
         */
        fun runViaLsp(project: Project, fqn: String, isTest: Boolean) {
            val command = if (isTest) "legend.runTest" else "legend.run"
            val server = findServer(project)
            if (server == null) {
                notifyServerMissing(project, fqn)
                return
            }
            runOnServer(server, command, listOf<Any?>(fqn), titleFor(command, fqn))
        }

        /**
         * `<<PCT.test>>` execution flow.
         *
         * 1. Ask the server for the discovered PCT adapters
         *    (`legend.listPctAdapters`). The list is sourced from
         *    the typed model — adding a new
         *    `<<PCT.adapter>>`-tagged Function in Pure source
         *    appears here on the next compile, no plugin work.
         * 2. Pop up a chooser populated with `(name, fqn)` rows.
         * 3. On selection, dispatch `legend.runPCT { fqn,
         *    adapterFqn }`. Server pulls in
         *    `rust_native_exclusions()` (same default as
         *    `legend test`) and runs the singleton PCT test.
         *
         * The popup uses the IDE's frame as the anchor when no
         * editor is active. Cancellation (Esc) just drops the
         * flow silently — no notification.
         */
        fun runPctViaLsp(project: Project, fqn: String) {
            val server = findServer(project)
            if (server == null) {
                notifyServerMissing(project, fqn)
                return
            }
            ApplicationManager.getApplication().executeOnPooledThread {
                val adapters = fetchPctAdapters(server)
                if (adapters.isEmpty()) {
                    notify(
                        project,
                        "Run PCT: $fqn",
                        "No PCT adapters were discovered in the workspace model. " +
                            "Define a function tagged with `<<PCT.adapter>>` + " +
                            "`{PCT.adapterName='<name>'}` in Pure source.",
                        NotificationType.ERROR,
                    )
                    return@executeOnPooledThread
                }
                ApplicationManager.getApplication().invokeLater {
                    showAdapterPopup(project, fqn, adapters) { chosen ->
                        runOnServer(
                            server,
                            "legend.runPCT",
                            listOf<Any?>(fqn, chosen.fqn),
                            "Run PCT: $fqn (${chosen.name})",
                        )
                    }
                }
            }
        }

        private fun notifyServerMissing(project: Project, fqn: String) {
            notify(
                project,
                fqn,
                "Legend Pure LSP server is not running. Open a `.pure` file to start it.",
                NotificationType.ERROR,
            )
        }

        private fun titleFor(command: String, fqn: String): String = when (command) {
            "legend.runTest" -> "Run test: $fqn"
            "legend.runPCT" -> "Run PCT: $fqn"
            else -> "Run: $fqn"
        }

        /** Discovered PCT adapter. */
        data class PctAdapter(val name: String, val fqn: String)

        private fun fetchPctAdapters(server: LspServer): List<PctAdapter> {
            val response: Any? = try {
                server.sendRequestSync(EXECUTE_TIMEOUT_MS) { lsp4j ->
                    lsp4j.workspaceService.executeCommand(
                        ExecuteCommandParams("legend.listPctAdapters", emptyList()),
                    )
                }
            } catch (e: Exception) {
                LOG.info("legend.listPctAdapters failed: ${e.message}")
                return emptyList()
            }
            val obj = parseJsonObject(response) ?: return emptyList()
            val extras = obj.get("extras") as? JsonArray ?: return emptyList()
            return extras.mapNotNull { entry ->
                val o = entry as? JsonObject ?: return@mapNotNull null
                val name = o.get("name")?.takeIf { it.isJsonPrimitive }?.asString ?: return@mapNotNull null
                val fqn = o.get("fqn")?.takeIf { it.isJsonPrimitive }?.asString ?: return@mapNotNull null
                PctAdapter(name = name, fqn = fqn)
            }
        }

        private fun showAdapterPopup(
            project: Project,
            fqn: String,
            adapters: List<PctAdapter>,
            onSelect: (PctAdapter) -> Unit,
        ) {
            val popup = JBPopupFactory.getInstance()
                .createPopupChooserBuilder(adapters)
                .setTitle("Pick PCT adapter for $fqn")
                .setRenderer { _, value, _, _, _ ->
                    javax.swing.JLabel("${value.name}  —  ${value.fqn}").apply {
                        isOpaque = false
                    }
                }
                .setItemChosenCallback { chosen -> onSelect(chosen) }
                .createPopup()
            // Anchor: prefer the frame's center so the popup is
            // visible even when the click came from a gutter icon
            // outside the focused editor.
            val frame = WindowManager.getInstance().getFrame(project)
            if (frame != null) {
                popup.showInCenterOf(frame.rootPane)
            } else {
                popup.showInFocusCenter()
            }
        }

        private fun findServer(project: Project): LspServer? {
            val mgr = LspServerManager.getInstance(project)
            // Make sure the server is at least asked to start (a
            // brand-new project with no `.pure` files open yet
            // wouldn't have one yet).
            mgr.startServersIfNeeded(PureLspServerSupportProvider::class.java)
            return mgr
                .getServersForProvider(PureLspServerSupportProvider::class.java)
                .firstOrNull()
        }

        /** Send `workspace/executeCommand` and surface the result.
         *  `arguments` is forwarded verbatim — each command has its
         *  own contract for what slots mean (`legend.run`/.../`.runTest`
         *  take `[fqn]`, `legend.runPCT` takes `[fqn, adapterFqn]`). */
        private fun runOnServer(
            server: LspServer,
            command: String,
            arguments: List<Any?>,
            title: String,
        ) {
            val project = server.project
            LOG.info("dispatching $command via LSP (${arguments.size} arg(s))")
            ApplicationManager.getApplication().executeOnPooledThread {
                val response: Any? = try {
                    server.sendRequestSync(EXECUTE_TIMEOUT_MS) { lsp4j ->
                        lsp4j.workspaceService.executeCommand(
                            ExecuteCommandParams(command, arguments),
                        )
                    }
                } catch (e: Exception) {
                    notify(project, title, "LSP request failed: ${e.message}", NotificationType.ERROR)
                    return@executeOnPooledThread
                }

                val parsed = parseExecuteCommandResult(response)
                val (body, type) = when {
                    parsed == null ->
                        "(unrecognized response shape: ${response?.javaClass?.name})" to
                            NotificationType.ERROR
                    parsed.ok ->
                        (parsed.value ?: "(no value)") to NotificationType.INFORMATION
                    else ->
                        (parsed.error ?: "(unknown error)") to NotificationType.ERROR
                }
                notify(project, title, body, type)
            }
        }

        /**
         * Parse the server's `ExecuteCommandResult` JSON shape into
         * a Kotlin record.
         *
         * lsp4j hands us back whatever Gson decodes `Object.class`
         * into: typically a `LinkedTreeMap<String, Any?>` (for raw
         * JSON objects), occasionally a `JsonObject` (when an
         * upstream adapter preserves the parse tree). Both shapes
         * must work — converting through `Gson().toJsonTree` re-roots
         * the value as a `JsonElement` regardless of which side it
         * came in on, and we then read the well-known fields.
         */
        private fun parseExecuteCommandResult(response: Any?): ExecuteCommandResult? {
            val obj = parseJsonObject(response) ?: return null
            val ok = obj.get("ok")?.takeIf { it.isJsonPrimitive }?.asBoolean ?: false
            val value = obj.get("value")
                ?.takeIf { it.isJsonPrimitive && it.asJsonPrimitive.isString }
                ?.asString
            val error = obj.get("error")
                ?.takeIf { it.isJsonPrimitive && it.asJsonPrimitive.isString }
                ?.asString
            return ExecuteCommandResult(ok = ok, value = value, error = error)
        }

        /**
         * Normalize lsp4j's `Object` response into a `JsonObject`.
         *
         * lsp4j parses `Object`-typed JSON-RPC results via Gson
         * default rules: JSON objects become `LinkedTreeMap`,
         * occasionally `JsonObject` when an upstream adapter keeps
         * the tree intact. Routing through `Gson().toJsonTree`
         * handles both — and `null` stays `null`.
         */
        private fun parseJsonObject(response: Any?): JsonObject? {
            if (response == null) return null
            return when (response) {
                is JsonObject -> response
                else -> Gson().toJsonTree(response) as? JsonObject
            }
        }

        private data class ExecuteCommandResult(
            val ok: Boolean,
            val value: String?,
            val error: String?,
        )

        private fun notify(project: Project, title: String, body: String, type: NotificationType) {
            NotificationGroupManager.getInstance()
                .getNotificationGroup(NOTIFICATION_GROUP)
                .createNotification(title, body, type)
                .notify(project)
        }
    }
}
