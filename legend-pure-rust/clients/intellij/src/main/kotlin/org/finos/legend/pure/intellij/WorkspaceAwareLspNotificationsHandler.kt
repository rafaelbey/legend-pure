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

import com.intellij.analysis.problemsView.ProblemsCollector
import com.intellij.analysis.problemsView.ProblemsProvider
import com.intellij.openapi.diagnostic.logger
import com.intellij.openapi.project.Project
import com.intellij.platform.lsp.api.LspServerDescriptor
import com.intellij.platform.lsp.api.LspServerNotificationsHandler
import com.intellij.problems.WolfTheProblemSolver
import org.eclipse.lsp4j.PublishDiagnosticsParams
import java.util.concurrent.ConcurrentHashMap

/**
 * Decorates the platform's [`LspServerNotificationsHandler`] so
 * `textDocument/publishDiagnostics` notifications surface for every
 * affected file in the project — not just the file the user is
 * currently editing.
 *
 * Why this exists
 * ---------------
 * IntelliJ's stock LSP integration renders diagnostics by funnelling
 * each `publishDiagnostics` notification into the per-editor
 * `MarkupModel` via [`LspDiagnosticsSupport`]. That pathway has no
 * effect for files the user hasn't opened in a tab: the diagnostic
 * is cached but never displayed in the Problems tool window, never
 * counted toward the project's problem badge, and never reachable
 * through "Recent Files" filters.
 *
 * For Legend Pure, a single edit (rename a class, narrow a
 * multiplicity, change a property type) can invalidate compile
 * across every file in the workspace that references the changed
 * element. The whole point of T-20260511-06 is to make that
 * fan-out visible — so we additionally route every
 * `publishDiagnostics` into:
 *
 *   1. **`ProblemsListener.TOPIC`** — what the Problems tool window's
 *      `ProblemsCollector` subscribes to. This is what actually
 *      makes files appear in the "Project Errors" tab.
 *   2. **`WolfTheProblemSolver`** — drives the red-filename badge in
 *      Project view and `isProblemFile` semantic queries. Requires
 *      a `problemFileHighlightFilter` extension to accept our file
 *      type ([`PureProblemFileHighlightFilter`]).
 *
 * Why all three pathways
 * ----------------------
 * We don't replace the platform's per-editor rendering — we add
 * to it. The original handler still gets called first, so files
 * the user has open continue to show inline squiggles and
 * scrollbar markers exactly as before. The `ProblemsListener` push
 * is what surfaces unopened files in the Problems tool window; the
 * Wolf push is what updates Project view's file-icon badges.
 *
 * State
 * -----
 * `activeProblems` tracks the currently-active
 * [`LegendPureFileProblem`] instances per URI. The Problems tool
 * window matches `problemAppeared` / `problemDisappeared` by
 * reference identity, so we must publish the *same* instance to
 * disappear that we earlier published to appear. The map is
 * `ConcurrentHashMap` because `publishDiagnostics` notifications
 * from the LSP4J runtime can arrive on different threads.
 *
 * Provider lifecycle
 * ------------------
 * `provider` identifies our problems to `ProblemsCollector`. It is
 * a `Disposable` whose disposal would cause the collector to drop
 * all our problems atomically. We don't dispose it explicitly —
 * the project's disposal cascades through the descriptor (which
 * holds the LSP server, which holds the `Lsp4jClient`, which holds
 * this wrapper).
 */
class WorkspaceAwareLspNotificationsHandler(
    private val delegate: LspServerNotificationsHandler,
    private val descriptor: LspServerDescriptor,
) : LspServerNotificationsHandler by delegate {

    private val project: Project = descriptor.project
    private val activeProblems: MutableMap<String, List<LegendPureFileProblem>> = ConcurrentHashMap()

    /**
     * Identity passed to every published `Problem`. Used by
     * `ProblemsCollector` to group problems by source plugin —
     * separates Legend Pure's reports from the Java compiler's,
     * Markdown's, etc.
     */
    private val provider = object : ProblemsProvider {
        override val project: Project = this@WorkspaceAwareLspNotificationsHandler.project
    }

    init {
        LOG.info("WorkspaceAwareLspNotificationsHandler installed for ${descriptor.presentableName}")
    }

    override fun publishDiagnostics(params: PublishDiagnosticsParams) {
        // 1. Platform pathway: cache + dispatch to any open editor.
        // Must run unconditionally so the user's current edit
        // continues to show inline squiggles.
        delegate.publishDiagnostics(params)

        try {
            val vfile = descriptor.findFileByUri(params.uri)
            if (vfile == null) {
                if (LOG.isDebugEnabled) {
                    LOG.debug("publishDiagnostics: vfile=null for uri=${params.uri}")
                }
                return
            }

            val diags = params.diagnostics ?: emptyList()

            // 2. ProblemsCollector pathway: feeds the project-wide
            // "Problems" tool window. Identity-tracked, so we must
            // publish `problemDisappeared` with the same instances
            // we earlier published with `problemAppeared`.
            val newProblems = diags.map { diag ->
                LegendPureFileProblem(
                    provider = provider,
                    file = vfile,
                    // LSP `Position` is 0-indexed; the Problems
                    // tool window expects 1-indexed display.
                    line = diag.range.start.line + 1,
                    column = diag.range.start.character + 1,
                    text = diag.message ?: "",
                )
            }
            val oldProblems = activeProblems.put(params.uri, newProblems) ?: emptyList()
            // Call the collector directly — `ProblemsListener.TOPIC`
            // has no platform-registered subscriber for this purpose
            // (the stock `ProblemsViewHighlightingWatcher` also calls
            // its stored listener instance directly, not via the
            // topic). `ProblemsCollector` is the project-level service
            // that backs the Problems tool window's "Project Errors"
            // tab via `CollectorBasedRoot`.
            val collector = ProblemsCollector.getInstance(project)
            oldProblems.forEach { collector.problemDisappeared(it) }
            newProblems.forEach { collector.problemAppeared(it) }

            // 3. Wolf pathway: red-filename badge in Project view +
            // `isProblemFile` queries. Requires a
            // `problemFileHighlightFilter` extension to clear Wolf's
            // `isToBeHighlighted` gate (see
            // `PureProblemFileHighlightFilter`).
            val wolf = WolfTheProblemSolver.getInstance(project)
            if (newProblems.isEmpty()) {
                wolf.clearProblems(vfile)
                activeProblems.remove(params.uri)
            } else {
                val wolfProblems = newProblems.map { p ->
                    wolf.convertToProblem(vfile, p.line, p.column, arrayOf(p.text))
                }
                wolf.reportProblems(vfile, wolfProblems)
            }

            if (LOG.isDebugEnabled) {
                LOG.debug(
                    "publishDiagnostics: ${newProblems.size} problems on ${vfile.name} " +
                        "(uri=${params.uri}, isProblemFile=${wolf.isProblemFile(vfile)})",
                )
            }
        } catch (e: Exception) {
            LOG.warn(
                "WorkspaceAware publishDiagnostics failed for uri=${params.uri}; " +
                    "per-editor diagnostics still rendered via delegate",
                e,
            )
        }
    }

    companion object {
        private val LOG = logger<WorkspaceAwareLspNotificationsHandler>()
    }
}
