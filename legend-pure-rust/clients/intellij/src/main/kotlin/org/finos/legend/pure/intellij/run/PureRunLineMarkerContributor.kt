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

import com.intellij.execution.lineMarker.RunLineMarkerContributor
import com.intellij.icons.AllIcons
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project
import com.intellij.psi.PsiElement
import org.finos.legend.pure.intellij.highlighting.PureTokenTypes

/**
 * Renders the ▶ gutter icon for runnable Pure functions.
 *
 * "Runnable" is whatever the LSP server's
 * `code_lenses_for(model, canonical_path)` says it is — that
 * function inspects `Function.parameters.is_empty()` and stereotype
 * membership directly on the typed `PureModel`, so we don't have to
 * re-decide from source text. This contributor just:
 *
 *  1. Fires once per `function` keyword leaf (cheap PSI filter).
 *  2. Maps the leaf to its line number.
 *  3. Asks [`LegendRunnableCache`] which lenses cover that line —
 *     the cache fronts a `textDocument/codeLens` request, sharded
 *     by document modification stamp.
 *  4. Returns an `Info` keyed off the lens's command and FQN.
 *
 * Why this exists vs. the LSP code-lens path: IntelliJ's stock
 * `com.intellij.platform.lsp` integration (still in 2025.3)
 * doesn't have a code-lens consumer, so the server's lenses go
 * unrendered. This is the same code path Kotlin / Java's `fun
 * main()` ▶ icon plugs into, applied to compiler-emitted lens
 * data. The server's lens emission stays useful for VS Code,
 * Neovim, etc. — and any future IntelliJ improvement picks it up
 * for free.
 */
class PureRunLineMarkerContributor : RunLineMarkerContributor(), DumbAware {

    override fun getInfo(element: PsiElement): Info? {
        // Cheap PSI filter: only fire on the `function` keyword
        // leaf. `getInfo` is called for every PSI element during
        // line-marker analysis; guarding here means we touch the
        // cache at most once per function declaration.
        val node = element.node ?: return null
        if (node.elementType != PureTokenTypes.KEYWORD) return null
        if (element.text != "function") return null

        val file = element.containingFile?.virtualFile ?: return null
        val document = element.containingFile?.viewProvider?.document ?: return null
        val line = document.getLineNumber(element.textRange.startOffset)

        val project = element.project
        val lenses = LegendRunnableCache.getInstance(project)
            .runnablesOnLine(file, line)
        if (lenses.isEmpty()) return null

        // Priority order when multiple lenses cover the same line:
        //   1. `legend.runPCT` — PCT functions have a distinct
        //      click flow (adapter-chooser popup → server-side
        //      `runPCTTests`). Strict precedence over the other
        //      two because PCT tests are never also plain tests or
        //      parameterless functions (they take an adapter
        //      parameter).
        //   2. `legend.runTest` — surveyor lifecycle hooks
        //      (`BeforePackage` / `AfterPackage` discovery).
        //   3. `legend.run` — direct call, no surveyor.
        val chosen = lenses.firstOrNull { it.command == "legend.runPCT" }
            ?: lenses.firstOrNull { it.command == "legend.runTest" }
            ?: lenses.first()
        val title = when (chosen.command) {
            "legend.runPCT" -> "Run PCT: ${chosen.fqn}"
            "legend.runTest" -> "Run Test: ${chosen.fqn}"
            else -> "Run ${chosen.fqn}"
        }
        return Info(
            AllIcons.RunConfigurations.TestState.Run,
            { _ -> title },
            RunPureFunctionAction(project, chosen.fqn, chosen.command),
        )
    }
}

/** Action invoked by the ▶ gutter icon. */
private class RunPureFunctionAction(
    private val project: Project,
    private val fqn: String,
    private val command: String,
) : AnAction(
    when (command) {
        "legend.runPCT" -> "Run PCT: $fqn"
        "legend.runTest" -> "Run Test: $fqn"
        else -> "Run $fqn"
    },
    null,
    AllIcons.RunConfigurations.TestState.Run,
), DumbAware {
    override fun actionPerformed(e: AnActionEvent) {
        // Route through the LSP server's `workspace/executeCommand`
        // — same path as a code-lens click. The server reuses the
        // already-compiled `PureModel` (open buffers + the
        // configured classpath cascade), so function dispatch sees
        // the user's classes, not just the embedded platform.
        when (command) {
            "legend.runPCT" -> PureRunCommandsSupport.runPctViaLsp(project, fqn)
            "legend.runTest" -> PureRunCommandsSupport.runViaLsp(project, fqn, isTest = true)
            else -> PureRunCommandsSupport.runViaLsp(project, fqn, isTest = false)
        }
    }
}
