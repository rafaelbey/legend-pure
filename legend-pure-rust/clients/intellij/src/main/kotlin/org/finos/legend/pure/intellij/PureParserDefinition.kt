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

import com.intellij.extapi.psi.ASTWrapperPsiElement
import com.intellij.extapi.psi.PsiFileBase
import com.intellij.lang.ASTNode
import com.intellij.lang.ParserDefinition
import com.intellij.lang.PsiBuilder
import com.intellij.lang.PsiParser
import com.intellij.lexer.Lexer
import com.intellij.openapi.project.Project
import com.intellij.psi.FileViewProvider
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiFile
import com.intellij.psi.tree.IElementType
import com.intellij.psi.tree.IFileElementType
import com.intellij.psi.tree.TokenSet
import org.finos.legend.pure.intellij.highlighting.PureLexer
import org.finos.legend.pure.intellij.highlighting.PureTokenTypes

/**
 * Minimal `ParserDefinition` for Pure files.
 *
 * The Pure semantic graph is owned by the Rust LSP server; the IDE
 * doesn't reparse Pure text. But `IntelliJ`'s GoTo Declaration
 * pipeline (the engine behind ⌘-hover underline + ⌘-click navigation)
 * needs *some* PSI tree to identify token boundaries: without it the
 * IDE has no way to know "the cursor is on the identifier `Person`"
 * vs "the cursor is in the gap between tokens", and short-circuits
 * before asking the LSP server.
 *
 * We satisfy that requirement with a *flat* PSI tree — every token
 * the lexer produces becomes a direct leaf under the file root, no
 * grammar structure. That is enough for the platform to (a) hand
 * the cursor position to the LSP server, (b) render error squiggles
 * over the right spans, and (c) drive ⌘-hover hints. Anything
 * grammar-shaped goes through the LSP server.
 */
class PureParserDefinition : ParserDefinition {

    override fun createLexer(project: Project?): Lexer = PureLexer()

    override fun getCommentTokens(): TokenSet = COMMENT_TOKENS

    override fun getStringLiteralElements(): TokenSet = STRING_TOKENS

    override fun createParser(project: Project?): PsiParser = FlatParser

    override fun getFileNodeType(): IFileElementType = FILE

    override fun createFile(viewProvider: FileViewProvider): PsiFile = PureFile(viewProvider)

    override fun createElement(node: ASTNode): PsiElement = ASTWrapperPsiElement(node)

    /**
     * Recursive-descent? No — flat. Walk every lexer token under the
     * file root marker. Done in one pass; no error recovery needed
     * because the LSP server reports diagnostics.
     */
    private object FlatParser : PsiParser {
        override fun parse(root: IElementType, builder: PsiBuilder): ASTNode {
            val rootMarker = builder.mark()
            while (!builder.eof()) {
                builder.advanceLexer()
            }
            rootMarker.done(root)
            return builder.treeBuilt
        }
    }

    companion object {
        @JvmField val FILE = IFileElementType("PURE_FILE", PureLanguage)
        private val COMMENT_TOKENS = TokenSet.create(
            PureTokenTypes.LINE_COMMENT,
            PureTokenTypes.BLOCK_COMMENT,
        )
        private val STRING_TOKENS = TokenSet.create(PureTokenTypes.STRING)
    }
}

class PureFile(viewProvider: FileViewProvider) :
    PsiFileBase(viewProvider, PureLanguage) {
    override fun getFileType() = PureFileType
    override fun toString() = "Pure file"
}
