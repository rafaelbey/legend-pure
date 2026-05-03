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

package com.finos.legend.pure.intellij.lsp

import com.intellij.openapi.project.Project
import com.redhat.devtools.lsp4ij.LanguageServerFactory
import com.redhat.devtools.lsp4ij.client.features.LSPClientFeatures
import com.redhat.devtools.lsp4ij.server.StreamConnectionProvider

/**
 * Wires the `legend lsp` Rust binary as the language server backing
 * `.pure` files. Registered in `plugin.xml` against the
 * [`com.redhat.devtools.lsp4ij.languageServer`] extension point.
 *
 * The settings UX (binary path, classpath descriptor, log level) is a
 * follow-up — for v1 the factory uses `legend` from `PATH` and lets
 * the LSP fall back to its discovery cascade for the classpath.
 */
class LegendPureLspServerFactory : LanguageServerFactory {

    override fun createConnectionProvider(project: Project): StreamConnectionProvider =
        LegendPureLspServerDescriptor(project)

    override fun createClientFeatures(): LSPClientFeatures = LSPClientFeatures()
}
