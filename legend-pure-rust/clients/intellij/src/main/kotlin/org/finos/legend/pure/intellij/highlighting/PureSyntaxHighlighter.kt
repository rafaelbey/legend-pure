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

import com.intellij.lexer.Lexer
import com.intellij.openapi.editor.DefaultLanguageHighlighterColors
import com.intellij.openapi.editor.HighlighterColors
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.editor.markup.TextAttributes
import com.intellij.openapi.fileTypes.SyntaxHighlighterBase
import com.intellij.psi.tree.IElementType
import com.intellij.ui.JBColor
import java.awt.Color
import java.awt.Font

/**
 * Maps each [PureTokenType] produced by [PureLexer] onto the
 * IntelliJ default colour scheme so users get usable colours
 * out-of-the-box without configuring a custom scheme.
 *
 * Each entry derives from a `DefaultLanguageHighlighterColors` key
 * — that anchors our highlighting to the user's active theme so
 * dark / light / custom schemes Just Work.
 */
class PureSyntaxHighlighter : SyntaxHighlighterBase() {
    override fun getHighlightingLexer(): Lexer = PureLexer()

    override fun getTokenHighlights(tokenType: IElementType?): Array<TextAttributesKey> {
        val key = TOKEN_KEYS[tokenType] ?: return EMPTY_KEYS
        return arrayOf(key)
    }

    companion object {
        private val EMPTY_KEYS = emptyArray<TextAttributesKey>()

        private val LINE_COMMENT = TextAttributesKey.createTextAttributesKey(
            "PURE_LINE_COMMENT", DefaultLanguageHighlighterColors.LINE_COMMENT)
        private val BLOCK_COMMENT = TextAttributesKey.createTextAttributesKey(
            "PURE_BLOCK_COMMENT", DefaultLanguageHighlighterColors.BLOCK_COMMENT)
        private val STRING = TextAttributesKey.createTextAttributesKey(
            "PURE_STRING", DefaultLanguageHighlighterColors.STRING)
        private val NUMBER = TextAttributesKey.createTextAttributesKey(
            "PURE_NUMBER", DefaultLanguageHighlighterColors.NUMBER)
        private val DATE = TextAttributesKey.createTextAttributesKey(
            "PURE_DATE", DefaultLanguageHighlighterColors.NUMBER)
        private val KEYWORD = TextAttributesKey.createTextAttributesKey(
            "PURE_KEYWORD", DefaultLanguageHighlighterColors.KEYWORD)
        private val BOOLEAN = TextAttributesKey.createTextAttributesKey(
            "PURE_BOOLEAN", DefaultLanguageHighlighterColors.KEYWORD)
        // Built-in M3 types (`String`, `Integer`, `Function`, …).
        // First we tried inheriting from CLASS_REFERENCE, then
        // PREDEFINED_SYMBOL — both rendered indistinguishably from
        // plain identifier text in many active themes. Here we pin
        // explicit attributes (bold + a JBColor that picks a sensible
        // teal for both light and dark schemes) so primitives are
        // visible regardless of the user's color-scheme setup. Users
        // can still override via *Settings → Editor → Color Scheme*
        // since we register a stable external name.
        private val BUILTIN_TYPE_ATTRS = TextAttributes(
            JBColor(Color(0x00_60_70), Color(0x4E_C9_B0)), // teal: dark in light, cyan in dark
            null, // background — keep theme default
            null, // effect color
            null, // effect type
            Font.BOLD,
        )
        private val BUILTIN_TYPE = TextAttributesKey.createTextAttributesKey(
            "PURE_BUILTIN_TYPE", BUILTIN_TYPE_ATTRS)
        // `<<my::Profile.Stereo>>` — METADATA matches the Java annotation
        // colour so stereotypes read like `@Annotation`.
        private val STEREOTYPE = TextAttributesKey.createTextAttributesKey(
            "PURE_STEREOTYPE", DefaultLanguageHighlighterColors.METADATA)
        private val IDENTIFIER = TextAttributesKey.createTextAttributesKey(
            "PURE_IDENTIFIER", DefaultLanguageHighlighterColors.IDENTIFIER)
        private val OPERATOR = TextAttributesKey.createTextAttributesKey(
            "PURE_OPERATOR", DefaultLanguageHighlighterColors.OPERATION_SIGN)
        private val BRACES = TextAttributesKey.createTextAttributesKey(
            "PURE_BRACES", DefaultLanguageHighlighterColors.BRACES)
        private val BRACKETS = TextAttributesKey.createTextAttributesKey(
            "PURE_BRACKETS", DefaultLanguageHighlighterColors.BRACKETS)
        private val PARENTHESES = TextAttributesKey.createTextAttributesKey(
            "PURE_PARENTHESES", DefaultLanguageHighlighterColors.PARENTHESES)
        private val DOT = TextAttributesKey.createTextAttributesKey(
            "PURE_DOT", DefaultLanguageHighlighterColors.DOT)
        private val SEMICOLON = TextAttributesKey.createTextAttributesKey(
            "PURE_SEMICOLON", DefaultLanguageHighlighterColors.SEMICOLON)
        private val COMMA = TextAttributesKey.createTextAttributesKey(
            "PURE_COMMA", DefaultLanguageHighlighterColors.COMMA)
        private val BAD_CHARACTER = TextAttributesKey.createTextAttributesKey(
            "PURE_BAD_CHARACTER", HighlighterColors.BAD_CHARACTER)

        private val TOKEN_KEYS: Map<IElementType, TextAttributesKey> = mapOf(
            PureTokenTypes.LINE_COMMENT  to LINE_COMMENT,
            PureTokenTypes.BLOCK_COMMENT to BLOCK_COMMENT,
            PureTokenTypes.STRING        to STRING,
            PureTokenTypes.NUMBER        to NUMBER,
            PureTokenTypes.DATE          to DATE,
            PureTokenTypes.KEYWORD       to KEYWORD,
            PureTokenTypes.BOOLEAN       to BOOLEAN,
            PureTokenTypes.BUILTIN_TYPE  to BUILTIN_TYPE,
            PureTokenTypes.STEREOTYPE    to STEREOTYPE,
            PureTokenTypes.IDENTIFIER    to IDENTIFIER,
            PureTokenTypes.OPERATOR      to OPERATOR,
            PureTokenTypes.BRACES        to BRACES,
            PureTokenTypes.BRACKETS      to BRACKETS,
            PureTokenTypes.PARENTHESES   to PARENTHESES,
            PureTokenTypes.DOT           to DOT,
            PureTokenTypes.SEMICOLON     to SEMICOLON,
            PureTokenTypes.COMMA         to COMMA,
            PureTokenTypes.BAD_CHARACTER to BAD_CHARACTER,
        )
    }
}
