// Copyright 2023 Goldman Sachs
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

package org.finos.legend.pure.runtime.java.extension.dsl.tds.interpreted;

import org.eclipse.collections.impl.tuple.Tuples;
import org.finos.legend.pure.runtime.java.extension.dsl.tds.interpreted.natives.StringToTDS;
import org.finos.legend.pure.runtime.java.extension.dsl.tds.interpreted.natives.TdsToCsv;
import org.finos.legend.pure.runtime.java.interpreted.extension.BaseInterpretedExtension;
import org.finos.legend.pure.runtime.java.interpreted.extension.InterpretedExtension;

public class TDSExtensionInterpreted extends BaseInterpretedExtension
{
    public TDSExtensionInterpreted()
    {
        super(
                Tuples.pair("stringToTDS_String_1__TDS_1_", StringToTDS::new),
                Tuples.pair("tdsToCsv_TDS_1__String_1_", TdsToCsv::new)
        );
    }

    public static InterpretedExtension extension()
    {
        return new TDSExtensionInterpreted();
    }
}
