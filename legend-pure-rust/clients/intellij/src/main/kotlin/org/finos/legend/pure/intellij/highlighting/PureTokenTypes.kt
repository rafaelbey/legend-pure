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
package org.finos.legend.pure.intellij.highlighting

import com.intellij.psi.tree.IElementType
import org.finos.legend.pure.intellij.PureLanguage

/**
 * Token classes consumed by [PureSyntaxHighlighter] and produced by
 * [PureLexer]. Intentionally coarse — finer categories (e.g.
 * "class name" vs "function name") need semantic context that the
 * client-side regex lexer can't supply; those will arrive later via
 * the LSP `textDocument/semanticTokens` request.
 */
object PureTokenTypes {
    @JvmField val LINE_COMMENT  = PureTokenType("LINE_COMMENT")
    @JvmField val BLOCK_COMMENT = PureTokenType("BLOCK_COMMENT")
    @JvmField val STRING        = PureTokenType("STRING")
    @JvmField val NUMBER        = PureTokenType("NUMBER")
    @JvmField val DATE          = PureTokenType("DATE")
    @JvmField val KEYWORD       = PureTokenType("KEYWORD")
    @JvmField val BOOLEAN       = PureTokenType("BOOLEAN")
    /** Built-in M3 type or primitive: `String`, `Integer`, `Function`, … */
    @JvmField val BUILTIN_TYPE  = PureTokenType("BUILTIN_TYPE")
    /** Stereotype block: `<<my::Profile.Stereo>>` including the angle brackets. */
    @JvmField val STEREOTYPE    = PureTokenType("STEREOTYPE")
    @JvmField val IDENTIFIER    = PureTokenType("IDENTIFIER")
    @JvmField val OPERATOR      = PureTokenType("OPERATOR")
    @JvmField val BRACES        = PureTokenType("BRACES")
    @JvmField val BRACKETS      = PureTokenType("BRACKETS")
    @JvmField val PARENTHESES   = PureTokenType("PARENTHESES")
    @JvmField val DOT           = PureTokenType("DOT")
    @JvmField val SEMICOLON     = PureTokenType("SEMICOLON")
    @JvmField val COMMA         = PureTokenType("COMMA")
    @JvmField val BAD_CHARACTER = PureTokenType("BAD_CHARACTER")
}

/** Single concrete `IElementType` subclass for all Pure tokens. */
class PureTokenType(debugName: String) : IElementType(debugName, PureLanguage)
