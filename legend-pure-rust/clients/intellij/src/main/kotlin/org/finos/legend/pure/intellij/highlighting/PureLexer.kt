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

import com.intellij.lexer.LexerBase
import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType

/**
 * Hand-rolled lexer that drives IntelliJ's syntax-highlighting
 * pipeline.
 *
 * Scope: cheap classification of each character range as comment /
 * string / number / keyword / identifier / punctuation. The token
 * shapes mirror the canonical lexer in `legend-pure-lexer/token.rs`
 * but lose all semantic awareness — `Class` is a keyword whether
 * it appears at the top level or inside a comment-stripped
 * identifier position. Anything richer (Class names vs function
 * names vs locals) requires the server's
 * `textDocument/semanticTokens`, deferred.
 *
 * No state is preserved across [advance] beyond what fits in
 * [getState] (returned as 0 here): IntelliJ may re-lex the buffer
 * from any byte offset on edits and our incrementality model is
 * "always start fresh." Adequate for typical file sizes; if it
 * ever bites perf, switch to a stateful lexer that publishes
 * "inside-block-comment" / "inside-string" via [getState].
 */
class PureLexer : LexerBase() {

    private var buffer: CharSequence = ""
    private var bufferEnd: Int = 0
    private var tokenStart: Int = 0
    private var tokenEnd: Int = 0
    private var tokenType: IElementType? = null

    override fun start(buffer: CharSequence, startOffset: Int, endOffset: Int, initialState: Int) {
        this.buffer = buffer
        this.bufferEnd = endOffset
        this.tokenStart = startOffset
        this.tokenEnd = startOffset
        advance()
    }

    override fun getState(): Int = 0
    override fun getTokenType(): IElementType? = tokenType
    override fun getTokenStart(): Int = tokenStart
    override fun getTokenEnd(): Int = tokenEnd
    override fun getBufferSequence(): CharSequence = buffer
    override fun getBufferEnd(): Int = bufferEnd

    override fun advance() {
        if (tokenEnd >= bufferEnd) {
            tokenType = null
            tokenStart = tokenEnd
            return
        }
        tokenStart = tokenEnd
        val c = buffer[tokenStart]

        when {
            c.isWhitespace() -> consumeWhile { it.isWhitespace() }.also { tokenType = TokenType.WHITE_SPACE }

            // Comments: `//` line, `/* */` block. Order matters: must
            // peek the second char before committing to OPERATOR `/`.
            c == '/' && peek(1) == '/' -> consumeLineComment()
            c == '/' && peek(1) == '*' -> consumeBlockComment()

            c == '\'' -> consumeString()

            // Stereotype block: `<<my::Profile.Stereo>>`. Eaten as a
            // single token spanning the angle brackets. Rendered with
            // the metadata color so it reads like a Java annotation.
            c == '<' && peek(1) == '<' -> consumeStereotype()

            // Date literal: `%2024-01-15`, `%2024-01-15T10:30:00`,
            // `%10:30:00`. We don't validate the inner shape — anything
            // up to the next non-digit/non-`-`/non-`:`/non-`T` ends it.
            c == '%' && peek(1)?.let { it.isDigit() || it == 'T' } == true -> consumeDate()

            c.isDigit() -> consumeNumber()

            // Negative numbers are handled by the parser, not the
            // highlighter — `-` always lexes as OPERATOR.
            isIdentStart(c) -> consumeIdentifierOrKeyword()

            c == '{' || c == '}' -> single(PureTokenTypes.BRACES)
            c == '[' || c == ']' -> single(PureTokenTypes.BRACKETS)
            c == '(' || c == ')' -> single(PureTokenTypes.PARENTHESES)
            c == '.' -> single(PureTokenTypes.DOT)
            c == ';' -> single(PureTokenTypes.SEMICOLON)
            c == ',' -> single(PureTokenTypes.COMMA)
            c == ':' && peek(1) == ':' -> twoChar(PureTokenTypes.OPERATOR)   // `::`
            c == '-' && peek(1) == '>' -> twoChar(PureTokenTypes.OPERATOR)   // `->`
            c == '=' && peek(1) == '=' -> twoChar(PureTokenTypes.OPERATOR)   // `==`
            c == '!' && peek(1) == '=' -> twoChar(PureTokenTypes.OPERATOR)   // `!=`
            c == '<' && peek(1) == '=' -> twoChar(PureTokenTypes.OPERATOR)   // `<=`
            c == '>' && peek(1) == '=' -> twoChar(PureTokenTypes.OPERATOR)   // `>=`
            c == '|' && peek(1) == '|' -> twoChar(PureTokenTypes.OPERATOR)   // `||`
            c == '&' && peek(1) == '&' -> twoChar(PureTokenTypes.OPERATOR)   // `&&`
            c in OPERATOR_CHARS -> single(PureTokenTypes.OPERATOR)

            else -> single(PureTokenTypes.BAD_CHARACTER)
        }
    }

    // ---------- token consumers ------------------------------------

    private fun consumeLineComment() {
        var i = tokenStart
        while (i < bufferEnd && buffer[i] != '\n') i++
        tokenEnd = i
        tokenType = PureTokenTypes.LINE_COMMENT
    }

    private fun consumeBlockComment() {
        var i = tokenStart + 2 // skip the opening `/*`
        while (i < bufferEnd - 1) {
            if (buffer[i] == '*' && buffer[i + 1] == '/') {
                i += 2
                tokenEnd = i
                tokenType = PureTokenTypes.BLOCK_COMMENT
                return
            }
            i++
        }
        // Unterminated — eat the rest of the buffer; still classify
        // as BLOCK_COMMENT so the user gets a visual cue.
        tokenEnd = bufferEnd
        tokenType = PureTokenTypes.BLOCK_COMMENT
    }

    private fun consumeString() {
        var i = tokenStart + 1 // skip the opening `'`
        while (i < bufferEnd) {
            val ch = buffer[i]
            if (ch == '\\' && i + 1 < bufferEnd) {
                i += 2 // escape sequence
                continue
            }
            if (ch == '\'') {
                tokenEnd = i + 1
                tokenType = PureTokenTypes.STRING
                return
            }
            if (ch == '\n') {
                // Strings don't span lines in Pure; close the token at
                // the newline so syntax errors stay localised.
                tokenEnd = i
                tokenType = PureTokenTypes.STRING
                return
            }
            i++
        }
        tokenEnd = bufferEnd
        tokenType = PureTokenTypes.STRING
    }

    private fun consumeDate() {
        var i = tokenStart + 1 // skip `%`
        while (i < bufferEnd) {
            val ch = buffer[i]
            if (ch.isDigit() || ch == '-' || ch == 'T' || ch == ':' || ch == '.') {
                i++
            } else break
        }
        tokenEnd = i
        tokenType = PureTokenTypes.DATE
    }

    private fun consumeNumber() {
        var i = tokenStart
        while (i < bufferEnd && buffer[i].isDigit()) i++
        // Optional fractional part.
        if (i + 1 < bufferEnd && buffer[i] == '.' && buffer[i + 1].isDigit()) {
            i++
            while (i < bufferEnd && buffer[i].isDigit()) i++
        }
        // Optional `D` decimal suffix.
        if (i < bufferEnd && buffer[i] == 'D') i++
        tokenEnd = i
        tokenType = PureTokenTypes.NUMBER
    }

    private fun consumeIdentifierOrKeyword() {
        var i = tokenStart
        while (i < bufferEnd && isIdentPart(buffer[i])) i++
        tokenEnd = i
        val text = buffer.subSequence(tokenStart, tokenEnd).toString()
        tokenType = when (text) {
            "true", "false" -> PureTokenTypes.BOOLEAN
            in KEYWORDS -> PureTokenTypes.KEYWORD
            in BUILTIN_TYPES -> PureTokenTypes.BUILTIN_TYPE
            else -> PureTokenTypes.IDENTIFIER
        }
    }

    private fun consumeStereotype() {
        // From the opening `<<`, scan forward for the matching `>>`.
        // If the closer never arrives (unterminated), stop at end-of-line
        // so a syntax error stays localised — same convention as
        // `consumeString`.
        var i = tokenStart + 2 // skip `<<`
        while (i < bufferEnd - 1) {
            if (buffer[i] == '>' && buffer[i + 1] == '>') {
                i += 2
                tokenEnd = i
                tokenType = PureTokenTypes.STEREOTYPE
                return
            }
            if (buffer[i] == '\n') {
                tokenEnd = i
                tokenType = PureTokenTypes.STEREOTYPE
                return
            }
            i++
        }
        tokenEnd = bufferEnd
        tokenType = PureTokenTypes.STEREOTYPE
    }

    // ---------- helpers --------------------------------------------

    private inline fun consumeWhile(pred: (Char) -> Boolean): Int {
        var i = tokenStart
        while (i < bufferEnd && pred(buffer[i])) i++
        tokenEnd = i
        return i
    }

    private fun peek(ahead: Int): Char? {
        val idx = tokenStart + ahead
        return if (idx < bufferEnd) buffer[idx] else null
    }

    private fun single(t: IElementType) {
        tokenEnd = tokenStart + 1
        tokenType = t
    }

    private fun twoChar(t: IElementType) {
        tokenEnd = tokenStart + 2
        tokenType = t
    }

    companion object {
        // Canonical keyword set sourced from
        // legend-pure-rust/crates/lexer/src/token.rs. `true`/`false`
        // are split out as BOOLEAN. Adjust here when the Rust lexer
        // adds new keywords; the IDE highlighter does not auto-sync.
        private val KEYWORDS = setOf(
            "Class", "Enum", "Profile", "Association", "Measure", "Primitive",
            "function", "import", "extends", "let", "native",
            "stereotypes", "tags", "shared", "composite", "none",
        )

        // Built-in M3 types and platform-shipped primitive / structural
        // types. Lexically indistinguishable from user identifiers, so
        // the lexer recognises them by exact name. Keep this aligned
        // with `legend-pure-core/legend-pure-m3-core/src/main/resources/platform/`
        // — primitives + the Function / Lambda hierarchy are stable;
        // user-introduced types in `meta::pure::*` are not in this list.
        private val BUILTIN_TYPES = setOf(
            // Primitives.
            "String", "Integer", "Boolean", "Float", "Decimal", "Number",
            "Date", "DateTime", "StrictDate", "StrictTime", "LatestDate",
            "Byte",
            // Roots / universal types.
            "Any", "Nil",
            // Function / lambda hierarchy.
            "Function", "LambdaFunction", "FunctionDefinition",
            "ConcreteFunctionDefinition", "NativeFunction",
            // Property / qualified property.
            "Property", "QualifiedProperty",
            // Type / class meta.
            "Type", "PrimitiveType", "GenericType",
        )

        private val OPERATOR_CHARS = setOf(
            '+', '-', '*', '/', '=', '<', '>', '!', '&', '|', '?', '^', '@', '~', ':',
            // `$` (variable-reference prefix) is treated as an operator
            // character so the lexer doesn't flag it as BAD_CHARACTER
            // (which renders red). No semantic meaning beyond that —
            // the user-facing effect is that `$abc` shows in plain
            // text, which is what we want.
            '$',
        )

        private fun isIdentStart(c: Char) = c.isLetter() || c == '_'
        private fun isIdentPart(c: Char) = c.isLetterOrDigit() || c == '_'
    }
}
