// Copyright 2026 Goldman Sachs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package com.finos.legend.pure.intellij

import com.intellij.lang.Language

/**
 * IntelliJ Language descriptor for Legend Pure (`.pure` files).
 *
 * The plugin ships no PSI parser of its own — semantic analysis is
 * delegated to the Rust LSP server via LSP4IJ. The Language object
 * is still required so file-type registration, syntax-color
 * preferences, and per-language editor settings have a place to hang
 * off.
 */
object LegendPureLanguage : Language("LegendPure", "text/legend-pure") {
    override fun isCaseSensitive(): Boolean = true
}
