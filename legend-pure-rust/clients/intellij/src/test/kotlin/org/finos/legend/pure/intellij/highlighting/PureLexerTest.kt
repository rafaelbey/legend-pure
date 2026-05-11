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
import com.intellij.testFramework.LexerTestCase

/**
 * Pins down the token stream the syntax highlighter sees. The
 * `LexerTestCase` harness drives [Lexer.start] / [Lexer.advance] for
 * us and emits a textual stream `<TOKEN_TYPE> ('<source>')` per
 * token, which we compare to a golden expectation. Drift between
 * the lexer rules and what the highlighter ships up is far the
 * easiest way to break colours; this test catches it.
 */
class PureLexerTest : LexerTestCase() {

    override fun createLexer(): Lexer = PureLexer()
    override fun getDirPath(): String = "" // no fixture dir; we feed sources inline.

    fun testRecognisesPrimitiveTypes() {
        // Each line below is a smoke check for one primitive — we
        // don't care about whitespace tokens here; the assertion looks
        // for the BUILTIN_TYPE class against the `String` / `Integer`
        // / etc. lexeme.
        val source = "name: String[1]; age: Integer[1]; ok: Boolean[1]; pi: Float[1];"
        val tokens = printTokens(source, 0)
        assertContainsTokenForLexeme(tokens, "BUILTIN_TYPE", "String")
        assertContainsTokenForLexeme(tokens, "BUILTIN_TYPE", "Integer")
        assertContainsTokenForLexeme(tokens, "BUILTIN_TYPE", "Boolean")
        assertContainsTokenForLexeme(tokens, "BUILTIN_TYPE", "Float")
    }

    fun testFunctionAndLambdaAreBuiltinTypes() {
        val source = "f: Function<{Any[*]->String[1]}>[1]; l: LambdaFunction<...>[*];"
        val tokens = printTokens(source, 0)
        assertContainsTokenForLexeme(tokens, "BUILTIN_TYPE", "Function")
        assertContainsTokenForLexeme(tokens, "BUILTIN_TYPE", "LambdaFunction")
    }

    fun testDollarSignNeverLexesAsBadCharacter() {
        // Variable references like `$abc` should NEVER trip the
        // BAD_CHARACTER fallback (which renders red). We don't give
        // `$` any special semantic role at the highlighting layer —
        // it just lexes as an OPERATOR character, and the following
        // identifier as a normal IDENTIFIER. End result: `$abc` shows
        // in default text colour, which is what the user prefers.
        // Raw / escaped because `$` is a Kotlin string-interpolation marker.
        val source = "\$abc + \$xyz123"
        val tokens = printTokens(source, 0)
        assertFalse(
            "`$` must not lex as BAD_CHARACTER:\n$tokens",
            tokens.contains("BAD_CHARACTER"),
        )
        assertContainsTokenForLexeme(tokens, "OPERATOR", "\$")
        assertContainsTokenForLexeme(tokens, "IDENTIFIER", "abc")
    }

    fun testStereotypeIsOneSpan() {
        val source = "<<my::Profile.Stereo>>"
        val tokens = printTokens(source, 0)
        assertContainsTokenForLexeme(tokens, "STEREOTYPE", "<<my::Profile.Stereo>>")
    }

    fun testUserIdentifierStaysIdentifier() {
        // Sanity: a non-builtin name like `Person` is still IDENTIFIER,
        // not BUILTIN_TYPE — proves we don't accidentally promote any
        // capitalised word to a built-in.
        val source = "Class test::Person {}"
        val tokens = printTokens(source, 0)
        assertContainsTokenForLexeme(tokens, "IDENTIFIER", "Person")
        // And `Class` IS a keyword.
        assertContainsTokenForLexeme(tokens, "KEYWORD", "Class")
    }

    /**
     * `LexerTestCase.printTokens` returns a multi-line string of the
     * shape `TOKEN_TYPE ('lexeme')` per token. We grep it instead of
     * comparing the whole stream so adding a new token between known
     * ones doesn't ripple into every assertion.
     */
    private fun assertContainsTokenForLexeme(
        tokenStream: String,
        expectedTokenType: String,
        expectedLexeme: String,
    ) {
        val needle = "$expectedTokenType ('$expectedLexeme')"
        assertTrue(
            "expected `$needle` in token stream:\n$tokenStream",
            tokenStream.contains(needle),
        )
    }
}
