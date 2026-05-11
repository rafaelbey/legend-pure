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

import com.intellij.openapi.fileTypes.LanguageFileType
import com.intellij.openapi.util.IconLoader
import javax.swing.Icon

object PureFileType : LanguageFileType(PureLanguage) {
    private val ICON: Icon = IconLoader.getIcon("/icons/pure-file.svg", PureFileType::class.java)

    override fun getName(): String = "Pure"
    override fun getDescription(): String = "Legend Pure source file"
    override fun getDefaultExtension(): String = "pure"
    override fun getIcon(): Icon = ICON
}
