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

import com.intellij.platform.lsp.api.customization.LspCommandsCustomizer
import com.intellij.platform.lsp.api.customization.LspCustomization

/**
 * [`LspCustomization`] with a [`PureRunCommandsSupport`] in the
 * `commandsCustomizer` slot.
 *
 * Override exists to surface the LSP server's response from
 * `workspace/executeCommand` as a `Notification`. The default
 * `LspCommandsSupport` fires the request but drops the response;
 * we want the user to see the rendered value / error / test
 * report inline, so we send the request ourselves via
 * `LspServer.sendRequestSync` and route the result through the
 * "Legend Pure" notification group. The PCT flow additionally
 * pops up an adapter chooser (`legend.listPctAdapters`) before
 * dispatching `legend.runPCT`.
 *
 * Other customizer slots (goto-def, hover, completion, …) keep
 * their defaults — same surface as the no-arg `LspCustomization()`
 * exposed before.
 */
class PureLspCustomization : LspCustomization() {
    override val commandsCustomizer: LspCommandsCustomizer = PureRunCommandsSupport()
}
