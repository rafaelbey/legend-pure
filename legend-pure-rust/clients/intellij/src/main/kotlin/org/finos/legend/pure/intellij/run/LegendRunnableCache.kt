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

import com.google.gson.JsonElement
import com.google.gson.JsonPrimitive
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.service
import com.intellij.openapi.diagnostic.logger
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspServer
import com.intellij.platform.lsp.api.LspServerManager
import org.eclipse.lsp4j.CodeLens
import org.eclipse.lsp4j.CodeLensParams
import org.finos.legend.pure.intellij.PureLspServerSupportProvider
import java.util.concurrent.ConcurrentHashMap

/**
 * Per-project cache of LSP `textDocument/codeLens` responses,
 * sharded by file + document modification stamp.
 *
 * The gutter ▶ contributor consults this cache instead of regexing
 * the declaration line — the LSP server's
 * `code_lenses_for(model, canonical_path)` is the compiler-driven
 * source of truth for "which functions are runnable" (it inspects
 * `Function.parameters.is_empty()` and stereotype membership
 * directly on the typed model, no text matching). Caching keyed on
 * the document mod stamp keeps us at one network-style RPC per
 * buffer-version: edits invalidate, idle viewers don't.
 *
 * Thread model: `runnablesOnLine` is called from background line-
 * marker analyzer threads (off-EDT). Cache access is guarded by a
 * `ConcurrentHashMap` for lookup and per-key fetch synchronization
 * by entering a single critical section per (file, modStamp) pair.
 */
@Service(Service.Level.PROJECT)
class LegendRunnableCache(private val project: Project) {

    /** One cached lens response. */
    data class Lens(
        /** 0-based zero-line index where the lens range starts. */
        val startLine: Int,
        /** 0-based zero-line index where the lens range ends, inclusive. */
        val endLine: Int,
        /** `legend.run` or `legend.runTest`. */
        val command: String,
        /** Function FQN that the command will dispatch on. */
        val fqn: String,
    ) {
        val isTest: Boolean get() = command == "legend.runTest"
    }

    private data class CacheEntry(
        val modStamp: Long,
        val lenses: List<Lens>,
    )

    private val byFile = ConcurrentHashMap<VirtualFile, CacheEntry>()

    /**
     * Return every cached lens that covers [line] in [file],
     * fetching from the LSP if the cache is missing or stale.
     *
     * `line` is 0-based to match LSP's coordinate system. Returns
     * an empty list when the LSP is not running yet (the IDE just
     * opened the file and the server is still warming up) — the
     * gutter will refresh on the next analyzer pass once the cache
     * fills in.
     */
    fun runnablesOnLine(file: VirtualFile, line: Int): List<Lens> {
        val entry = getOrFetch(file) ?: return emptyList()
        return entry.lenses.filter { line in it.startLine..it.endLine }
    }

    private fun getOrFetch(file: VirtualFile): CacheEntry? {
        val currentStamp = currentModStamp(file) ?: return null
        val cached = byFile[file]
        if (cached != null && cached.modStamp == currentStamp) {
            return cached
        }
        // Single concurrent fetch per file: lock once, re-check
        // inside, then issue the request. Lock granularity is the
        // file's VirtualFile identity — different files refresh
        // concurrently.
        synchronized(fetchLockFor(file)) {
            val recheck = byFile[file]
            if (recheck != null && recheck.modStamp == currentStamp) {
                return recheck
            }
            val server = lspServer() ?: return null
            val lenses = fetchLenses(server, file)
            val fresh = CacheEntry(modStamp = currentStamp, lenses = lenses)
            byFile[file] = fresh
            return fresh
        }
    }

    private fun fetchLenses(server: LspServer, file: VirtualFile): List<Lens> {
        val docId = try {
            server.getDocumentIdentifier(file)
        } catch (e: Exception) {
            LOG.info("getDocumentIdentifier failed for ${file.path}: ${e.message}")
            return emptyList()
        }
        val params = CodeLensParams(docId)
        val response: List<CodeLens>? = try {
            server.sendRequestSync(FETCH_TIMEOUT_MS) { lsp4j ->
                lsp4j.textDocumentService.codeLens(params)
            }
        } catch (e: Exception) {
            LOG.info("textDocument/codeLens failed for ${file.path}: ${e.message}")
            return emptyList()
        }
        return response.orEmpty().mapNotNull { toLens(it) }
    }

    private fun toLens(cl: CodeLens): Lens? {
        val command = cl.command ?: return null
        if (command.command !in RUNNABLE_COMMANDS) {
            return null
        }
        // lsp4j parses arguments as a List<Object> where each item
        // may already be a String or still a raw `JsonElement`
        // (depends on which message-handler path Gson took). Tolerate
        // both shapes.
        val arg = command.arguments?.firstOrNull() ?: return null
        val fqn = when (arg) {
            is String -> arg
            is JsonPrimitive -> if (arg.isString) arg.asString else arg.toString()
            is JsonElement -> arg.toString().trim('"')
            else -> arg.toString()
        }
        if (fqn.isBlank()) return null
        val range = cl.range ?: return null
        return Lens(
            startLine = range.start.line,
            endLine = range.end.line,
            command = command.command,
            fqn = fqn,
        )
    }

    private fun lspServer(): LspServer? =
        LspServerManager.getInstance(project)
            .getServersForProvider(PureLspServerSupportProvider::class.java)
            .firstOrNull()

    /** Map a VirtualFile to a stable lock token. */
    private val fetchLocks = ConcurrentHashMap<VirtualFile, Any>()
    private fun fetchLockFor(file: VirtualFile): Any =
        fetchLocks.computeIfAbsent(file) { Any() }

    private fun currentModStamp(file: VirtualFile): Long? {
        val document = FileDocumentManager.getInstance().getDocument(file)
        return document?.modificationStamp ?: file.modificationStamp
    }

    companion object {
        private val LOG = logger<LegendRunnableCache>()

        /** Per-fetch timeout. CodeLens is recomputed off the in-memory
         *  `PureModel` and is fast; 2s is plenty even for cold
         *  workspaces. */
        private const val FETCH_TIMEOUT_MS = 2_000

        /** LSP `Command.command` values the gutter recognizes. Lenses
         *  with any other command are filtered out at parse time. */
        private val RUNNABLE_COMMANDS = setOf(
            "legend.run",
            "legend.runTest",
            "legend.runPCT",
        )

        fun getInstance(project: Project): LegendRunnableCache = project.service()
    }
}
