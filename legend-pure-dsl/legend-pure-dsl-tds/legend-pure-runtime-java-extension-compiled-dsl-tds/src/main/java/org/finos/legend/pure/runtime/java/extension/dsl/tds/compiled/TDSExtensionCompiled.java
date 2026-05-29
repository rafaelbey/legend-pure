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

package org.finos.legend.pure.runtime.java.extension.dsl.tds.compiled;

import java.util.List;

import org.eclipse.collections.api.block.function.Function3;
import org.eclipse.collections.api.factory.Lists;
import org.eclipse.collections.api.list.ListIterable;
import org.finos.legend.pure.m3.navigation.Instance;
import org.finos.legend.pure.m3.navigation.M3Paths;
import org.finos.legend.pure.m3.navigation.M3Properties;
import org.finos.legend.pure.m3.navigation.ProcessorSupport;
import org.finos.legend.pure.m4.coreinstance.CoreInstance;
import org.finos.legend.pure.runtime.java.compiled.extension.CompiledExtension;
import org.finos.legend.pure.runtime.java.compiled.generation.ProcessorContext;
import org.finos.legend.pure.runtime.java.compiled.generation.processors.type.TypeProcessor;
import org.finos.legend.pure.runtime.java.compiled.generation.processors.natives.Native;
import org.finos.legend.pure.runtime.java.compiled.generation.processors.valuespecification.ValueSpecificationProcessor;
import org.finos.legend.pure.runtime.java.extension.dsl.tds.compiled.natives.StringToTDS;
import org.finos.legend.pure.runtime.java.extension.dsl.tds.compiled.natives.TdsToCsv;

public class TDSExtensionCompiled implements CompiledExtension
{
    private static final String TDS_TUPLE_SUPPORT = "org.finos.legend.pure.runtime.java.extension.dsl.tds.compiled.TDSTupleSupport";

    @Override
    public List<Native> getExtraNatives()
    {
        return Lists.fixedSize.with(
            new StringToTDS(),
            new TdsToCsv()
        );
    }

    @Override
    public String getRelatedRepository()
    {
        return "platform_dsl_tds";
    }

    /**
     * Codegen hook for `$row.colName` (Column-as-function application) on
     * TDSTuple-backed rows.
     *
     * Pure parses `$row.colName` as a function application whose function
     * is a `Column` instance. Without a hook, the compiled engine has no
     * generated getter for an arbitrary column name. Our row's static
     * type is `T` (a RelationType — set by `TDSExtension.parse`'s
     * classifier override), so we discriminate by receiver static type:
     * if the function is a Column AND the receiver's static rawType is a
     * RelationType, we generate a positional read of the cell at the
     * column's index via {@link TDSTupleSupport#cellAt}. The index is
     * resolved at codegen time from the receiver's RelationType columns.
     *
     * Returns {@code null} for any non-RelationType receiver, so other
     * extensions (legend-engine's RowContainer-based hook) can win for
     * their own row representations. The SPI errors if more than one
     * extension returns non-null — both sides need to stay receiver-scoped
     * for coexistence (see plan).
     */
    @Override
    public Function3<CoreInstance, CoreInstance, ProcessorContext, String> getExtraFunctionGeneration()
    {
        return (CoreInstance function, CoreInstance functionExpression, ProcessorContext processorContext) ->
        {
            ProcessorSupport ps = processorContext.getSupport();
            if (!ps.instance_instanceOf(function, M3Paths.Column))
            {
                return null;
            }

            ListIterable<? extends CoreInstance> args = Instance.getValueForMetaPropertyToManyResolved(functionExpression, M3Properties.parametersValues, ps);
            if (args.isEmpty())
            {
                return null;
            }
            CoreInstance firstParam = args.getFirst();
            CoreInstance receiverGT = Instance.getValueForMetaPropertyToOneResolved(firstParam, M3Properties.genericType, ps);
            CoreInstance rawType = (receiverGT == null) ? null : Instance.getValueForMetaPropertyToOneResolved(receiverGT, M3Properties.rawType, ps);
            if (rawType == null || !ps.instance_instanceOf(rawType, M3Paths.RelationType))
            {
                return null;
            }

            // Resolve the column index at codegen time.
            CoreInstance columnNameCI = function.getValueForMetaPropertyToOne(M3Properties.name);
            String colName = (columnNameCI == null) ? null : columnNameCI.getName();
            if (colName == null)
            {
                return null;
            }
            ListIterable<? extends CoreInstance> cols = Instance.getValueForMetaPropertyToManyResolved(rawType, M3Properties.columns, ps);
            int index = -1;
            for (int i = 0; i < cols.size(); i++)
            {
                CoreInstance nameCI = cols.get(i).getValueForMetaPropertyToOne(M3Properties.name);
                if (nameCI != null && colName.equals(nameCI.getName()))
                {
                    index = i;
                    break;
                }
            }
            if (index < 0)
            {
                return null;
            }

            // Generate `(returnType) TDSTupleSupport.cellAt((CoreInstance) receiver, index, "<typeName>")`.
            // The typeName arg is the column's Pure primitive type — passed
            // as a literal so cellAt can pick the right unboxing without
            // calling getClassifier() on the cell value (compiled-mode
            // ValCoreInstance forbids that).
            String processedReceiver = ValueSpecificationProcessor.processValueSpecification(null, firstParam, processorContext);
            CoreInstance nativeFunction = Instance.getValueForMetaPropertyToOneResolved(functionExpression, M3Properties.func, ps);
            CoreInstance functionType = ps.function_getFunctionType(nativeFunction);
            CoreInstance returnGT = Instance.getValueForMetaPropertyToOneResolved(functionType, M3Properties.returnType, ps);
            String returnType = TypeProcessor.typeToJavaObjectSingle(returnGT, true, ps);
            CoreInstance returnRawType = (returnGT == null) ? null : Instance.getValueForMetaPropertyToOneResolved(returnGT, M3Properties.rawType, ps);
            String columnTypeName = (returnRawType == null) ? "String" : returnRawType.getName();
            return "(" + returnType + ")" + TDS_TUPLE_SUPPORT + ".cellAt((org.finos.legend.pure.m4.coreinstance.CoreInstance)(" + processedReceiver + "), " + index + ", \"" + columnTypeName + "\")";
        };
    }

    public static CompiledExtension extension()
    {
        return new TDSExtensionCompiled();
    }
}
