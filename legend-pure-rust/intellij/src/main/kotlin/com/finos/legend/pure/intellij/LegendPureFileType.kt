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
import com.intellij.openapi.fileTypes.LanguageFileType
import javax.swing.Icon

/** `.pure` source file type — text, editable. */
object LegendPureFileType : LanguageFileType(LegendPureLanguage) {
    override fun getName(): String = "LegendPure"
    override fun getDescription(): String = "Legend Pure language source"
    override fun getDefaultExtension(): String = "pure"
    override fun getIcon(): Icon = AllIcons.FileTypes.Text
}
