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

import com.intellij.icons.AllIcons
import com.intellij.openapi.fileTypes.FileType
import com.intellij.openapi.vfs.VirtualFile
import javax.swing.Icon

/**
 * `.purem` archive file type — a binary slice produced by
 * `legend-pure-build`. Marked binary so IntelliJ doesn't try to load
 * it into a text editor by default.
 *
 * A future `LegendPuremFileEditorProvider` (see Step 5d in
 * `do-we-have-enought-quiet-swing.md`) will decompile this into a
 * read-only Pure-source view via the `legend emit` CLI; for now
 * IntelliJ just labels it and surfaces it in the project tree.
 */
object LegendPuremFileType : FileType {
    override fun getName(): String = "LegendPurem"
    override fun getDescription(): String = "Legend Pure compiled archive"
    override fun getDefaultExtension(): String = "purem"
    override fun getIcon(): Icon = AllIcons.FileTypes.Archive
    override fun isBinary(): Boolean = true
    override fun isReadOnly(): Boolean = true
    override fun getCharset(file: VirtualFile, content: ByteArray): String? = null
}
