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
import com.intellij.notification.Notification
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationListener
import com.intellij.notification.NotificationType
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.diagnostic.logger
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.fileEditor.OpenFileDescriptor
import com.intellij.openapi.project.Project
import com.intellij.openapi.roots.ProjectRootManager
import com.intellij.openapi.ui.popup.JBPopupFactory
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.openapi.wm.WindowManager
import javax.swing.event.HyperlinkEvent
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
                if (parsed == null) {
                    notify(
                        project,
                        title,
                        "(unrecognized response shape: ${response?.javaClass?.name})",
                        NotificationType.ERROR,
                    )
                    return@executeOnPooledThread
                }
                val type = if (parsed.ok) NotificationType.INFORMATION else NotificationType.ERROR
                val plain = (if (parsed.ok) parsed.value else parsed.error) ?: "(no value)"
                // Mirror the result into the "Legend Pure Run"
                // tool window so it's persistent (scrolls, copy,
                // multiple tabs) — the balloon stays as a
                // transient one-line summary.
                openInToolWindow(project, title, parsed)
                notify(project, title, plain, type, parsed.failures)
            }
        }

        /** Render the LSP response in the Pure Run tool window. */
        private fun openInToolWindow(
            project: Project,
            title: String,
            parsed: ExecuteCommandResult,
        ) {
            val headerType = if (parsed.ok) {
                PureRunToolWindowService.HeaderType.SUCCESS
            } else {
                PureRunToolWindowService.HeaderType.FAILURE
            }
            // `value` carries the test-report summary or rendered
            // return value; `error` carries the failure summary
            // for run/PCT errors. Either way the first line is
            // the header — strip it from the body so we don't
            // double-print the failure rows (which we re-render
            // structurally below).
            val rawText = (if (parsed.ok) parsed.value else parsed.error) ?: ""
            val header = rawText.lineSequence().firstOrNull().orEmpty().ifEmpty { title }
            // The "success value" path is reserved for runs that
            // returned without a structured failure list (i.e.
            // `legend.run` on a function that completed). Tests
            // produce a populated `failures` even on the green
            // path — there we let the header carry the counts
            // and skip the redundant "returned" line.
            val successValue =
                if (parsed.ok && parsed.failures.isEmpty()) parsed.value else null
            val failures = parsed.failures.map { f ->
                PureRunToolWindowService.FailureRender(
                    fqn = f.fqn,
                    message = f.message,
                    stack = f.stack.map { s ->
                        PureRunToolWindowService.FrameRender(
                            name = s.name,
                            source = s.source,
                            line = s.line,
                            column = s.column,
                        )
                    },
                )
            }
            PureRunToolWindowService.getInstance(project).showRun(
                title = title,
                render = PureRunToolWindowService.RunRender(
                    header = header,
                    headerType = headerType,
                    successValue = successValue,
                    failures = failures,
                    stdout = parsed.stdout,
                ),
            )
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
            val extras = obj.get("extras") as? JsonObject
            val failures = extras?.get("failures") as? JsonArray
            val parsedFailures = failures?.mapNotNull { entry ->
                val o = entry as? JsonObject ?: return@mapNotNull null
                val stack = (o.get("stack") as? JsonArray)?.mapNotNull { frameEntry ->
                    val f = frameEntry as? JsonObject ?: return@mapNotNull null
                    StackFrame(
                        name = f.get("name")?.takeIf { it.isJsonPrimitive }?.asString ?: "",
                        source = f.get("source")?.takeIf { it.isJsonPrimitive }?.asString ?: "",
                        line = f.get("line")?.takeIf { it.isJsonPrimitive }?.asInt ?: 1,
                        column = f.get("column")?.takeIf { it.isJsonPrimitive }?.asInt ?: 1,
                    )
                }.orEmpty()
                FailureDetail(
                    fqn = o.get("fqn")?.takeIf { it.isJsonPrimitive }?.asString ?: "",
                    message = o.get("message")?.takeIf { it.isJsonPrimitive }?.asString ?: "",
                    stack = stack,
                )
            }.orEmpty()
            val stdout = extras?.get("stdout")
                ?.takeIf { it.isJsonPrimitive && it.asJsonPrimitive.isString }
                ?.asString
                .orEmpty()
            return ExecuteCommandResult(
                ok = ok,
                value = value,
                error = error,
                failures = parsedFailures,
                stdout = stdout,
            )
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
            val failures: List<FailureDetail> = emptyList(),
            /** Captured Pure-level `print` / `println` output, in
             *  emission order. Empty when the run produced no
             *  console writes. */
            val stdout: String = "",
        )

        /** One failing run / test, with the PureException's full
         *  call stack so the IDE can offer a clickable link per
         *  frame. Frames are in innermost-first order — the first
         *  frame is where the exception actually fired. */
        private data class FailureDetail(
            val fqn: String,
            val message: String,
            val stack: List<StackFrame>,
        )

        /** One frame of a PureException's call stack. */
        private data class StackFrame(
            val name: String,
            val source: String,
            val line: Int,
            val column: Int,
        )

        private fun notify(
            project: Project,
            title: String,
            body: String,
            type: NotificationType,
            failures: List<FailureDetail> = emptyList(),
        ) {
            val htmlBody = renderHtmlBody(body, failures)
            val n = NotificationGroupManager.getInstance()
                .getNotificationGroup(NOTIFICATION_GROUP)
                .createNotification(title, htmlBody, type)
            if (failures.any { it.stack.isNotEmpty() }) {
                // The body contains `<a href="legend-pure-failure?...">`
                // links per failure with source info. The listener
                // intercepts the click, parses the URL's params,
                // and opens the file at the failure's line/column.
                n.setListener(FailureLinkListener(project))
            }
            n.notify(project)
        }

        /**
         * Build the notification body as HTML. The top section is
         * always the plain-text summary (counts, return value,
         * error). When `failures` is non-empty we re-emit per-test
         * rows as clickable links — they replace the equivalent
         * server-rendered rows in `body`, since the latter is
         * already text and would render twice if we appended.
         *
         * HTML metacharacters in the body and in user-facing
         * fields are escaped so an assertion message that contains
         * `<` or `&` doesn't break the layout.
         */
        private fun renderHtmlBody(body: String, failures: List<FailureDetail>): String {
            if (failures.isEmpty()) {
                return "<html>${escapeHtml(body).replace("\n", "<br/>")}</html>"
            }
            // Keep the body's header line (e.g. test counts, or
            // `[legend.run] Execution error`) and drop the rest —
            // we re-emit the failure detail with clickable
            // per-frame links below, replacing whatever
            // server-rendered text the body carried.
            val header = body.lineSequence().firstOrNull().orEmpty()
            val sb = StringBuilder("<html>")
            sb.append(escapeHtml(header))
            for (f in failures.take(5)) {
                sb.append("<br/>&nbsp;&nbsp;✗ ")
                val firstFrame = f.stack.firstOrNull()
                if (firstFrame != null) {
                    sb.append("<a href=\"")
                    sb.append(buildFrameUrl(firstFrame))
                    sb.append("\">")
                    sb.append(escapeHtml(f.fqn))
                    sb.append("</a>")
                } else {
                    sb.append(escapeHtml(f.fqn))
                }
                if (f.message.isNotEmpty()) {
                    sb.append("<br/>&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;")
                    sb.append(escapeHtml(f.message))
                }
                // One link per stack frame — innermost first, so a
                // click on the first row jumps to the actual
                // failure site (e.g. the `toOne` call) and
                // subsequent rows walk back up the call chain.
                for ((idx, frame) in f.stack.withIndex()) {
                    if (idx >= 8) {
                        sb.append("<br/>&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;")
                        sb.append("<span style='color:gray'>… ${f.stack.size - 8} more frame(s)</span>")
                        break
                    }
                    sb.append("<br/>&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;at <a href=\"")
                    sb.append(buildFrameUrl(frame))
                    sb.append("\">")
                    sb.append(escapeHtml(frame.name))
                    sb.append("</a> <span style='color:gray'>(")
                    sb.append(escapeHtml(frame.source))
                    sb.append(":")
                    sb.append(frame.line)
                    sb.append(")</span>")
                }
            }
            if (failures.size > 5) {
                sb.append("<br/>&nbsp;&nbsp;… and ${failures.size - 5} more")
            }
            sb.append("</html>")
            return sb.toString()
        }

        /** Synthesize the click-target URL for a stack frame. The
         *  scheme is bespoke (`legend-pure-failure://`) so the
         *  `FailureLinkListener` can recognize it without
         *  colliding with `http(s)` links. */
        private fun buildFrameUrl(frame: StackFrame): String {
            val src = java.net.URLEncoder.encode(frame.source, "UTF-8")
            return "legend-pure-failure://nav?source=$src&line=${frame.line}&column=${frame.column}"
        }

        private fun escapeHtml(s: String): String = s
            .replace("&", "&amp;")
            .replace("<", "&lt;")
            .replace(">", "&gt;")
    }
}

/**
 * Intercepts `<a href="legend-pure-failure://...">` clicks in the
 * test-result notification and navigates to the failure's source
 * line/column.
 *
 * The `source` field as it comes from the runtime is a "canonical"
 * path (e.g. `/myproj/foo.pure` — what the LSP server uses
 * internally for the compiled `PureModel`). It may or may not
 * match a real absolute filesystem path, so we resolve in three
 * stages:
 *
 *  1. As-is via `LocalFileSystem` (handles real absolute paths).
 *  2. As a relative path under the project base.
 *  3. Suffix-match against any source root in the project — same
 *     fallback IntelliJ uses for "jump to file by name".
 *
 * The first hit wins; if none do, we drop a `WARNING` notification
 * with the path so the user can see why navigation failed.
 */
private class FailureLinkListener(private val project: Project) : NotificationListener {
    override fun hyperlinkUpdate(notification: Notification, event: HyperlinkEvent) {
        if (event.eventType != HyperlinkEvent.EventType.ACTIVATED) return
        val url = event.url?.toString() ?: event.description ?: return
        if (!url.startsWith("legend-pure-failure://")) return
        val params = parseQuery(url)
        val source = params["source"] ?: return
        val line = params["line"]?.toIntOrNull() ?: 1
        val column = params["column"]?.toIntOrNull() ?: 1
        val file = resolveFile(source)
        if (file == null) {
            NotificationGroupManager.getInstance()
                .getNotificationGroup("Legend Pure")
                .createNotification(
                    "Couldn't find $source in project",
                    "Verify the source path resolves under one of the project's roots.",
                    NotificationType.WARNING,
                )
                .notify(project)
            return
        }
        // `OpenFileDescriptor` wants 0-based line/column; LSP /
        // PureException are both 1-based.
        OpenFileDescriptor(project, file, (line - 1).coerceAtLeast(0), (column - 1).coerceAtLeast(0))
            .navigate(true)
    }

    private fun resolveFile(source: String): VirtualFile? {
        val lfs = LocalFileSystem.getInstance()
        lfs.findFileByPath(source)?.let { return it }
        val base = project.basePath
        if (base != null) {
            lfs.findFileByPath("$base/${source.trimStart('/')}")?.let { return it }
        }
        // Suffix-match across source roots — last-resort fallback
        // when the canonical path doesn't map directly onto the
        // filesystem (e.g. classpath-resolved files).
        val suffix = source.trimStart('/')
        for (root in ProjectRootManager.getInstance(project).contentSourceRoots) {
            val candidate = root.findFileByRelativePath(suffix)
            if (candidate != null) return candidate
        }
        return null
    }

    private fun parseQuery(url: String): Map<String, String> {
        val q = url.substringAfter('?', "")
        if (q.isEmpty()) return emptyMap()
        return q.split('&').mapNotNull {
            val (k, v) = it.split('=', limit = 2).let { p ->
                if (p.size == 2) p[0] to p[1] else return@mapNotNull null
            }
            k to java.net.URLDecoder.decode(v, "UTF-8")
        }.toMap()
    }
}
