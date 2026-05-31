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

import java.util.Stack;

import org.eclipse.collections.api.list.ListIterable;
import org.eclipse.collections.api.map.MutableMap;
import org.eclipse.collections.api.stack.MutableStack;
import org.eclipse.collections.impl.tuple.Tuples;
import org.finos.legend.pure.m3.coreinstance.meta.pure.metamodel.function.Function;
import org.finos.legend.pure.m3.navigation.Instance;
import org.finos.legend.pure.m3.navigation.M3Paths;
import org.finos.legend.pure.m3.navigation.M3Properties;
import org.finos.legend.pure.m3.navigation.ProcessorSupport;
import org.finos.legend.pure.m3.navigation.ValueSpecificationBootstrap;
import org.finos.legend.pure.m3.navigation.relation._Column;
import org.finos.legend.pure.m3.navigation.type.Type;
import org.finos.legend.pure.m4.coreinstance.CoreInstance;
import org.finos.legend.pure.runtime.java.extension.dsl.tds.interpreted.natives.StringToTDS;
import org.finos.legend.pure.runtime.java.extension.dsl.tds.interpreted.natives.TdsToCsv;
import org.finos.legend.pure.runtime.java.interpreted.ExecutionSupport;
import org.finos.legend.pure.runtime.java.interpreted.FunctionExecutionInterpreted;
import org.finos.legend.pure.runtime.java.interpreted.VariableContext;
import org.finos.legend.pure.runtime.java.interpreted.extension.BaseInterpretedExtension;
import org.finos.legend.pure.runtime.java.interpreted.extension.InterpretedExtension;
import org.finos.legend.pure.runtime.java.interpreted.natives.InstantiationContext;
import org.finos.legend.pure.runtime.java.interpreted.natives.essentials.lang.cast.Cast;
import org.finos.legend.pure.runtime.java.interpreted.profiler.Profiler;

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

    /**
     * Receiver-scoped Column-application hook for TDSTuple rows.
     *
     * `$row.colName` lowers to a function application where the function is
     * a Column. When the receiver is a `TDSTuple` (rows produced by
     * `stringToTDS` / `#TDS#` literals — see {@code TDSExtension.parse}),
     * we read the cell positionally from {@code row.values} (a
     * {@code List<Any>[*]} where each inner List has 0 or 1 elements).
     * The column's position is resolved against the row's overridden
     * {@code classifierGenericType.rawType} (the RelationType T, set by
     * the parse so Pure-level typing still sees the row as an instance
     * of T even though the Java backing class is TDSTuple).
     *
     * Returns {@code null} for any non-TDSTuple receiver, so legend-engine's
     * RowContainer-based hook still wins for relation rows it owns —
     * provided both hooks stay receiver-scoped (the SPI errors on
     * multiple non-null results, by design).
     */
    @Override
    public CoreInstance getExtraFunctionExecution(Function<?> function, ListIterable<? extends CoreInstance> params, Stack<MutableMap<String, CoreInstance>> resolvedTypeParameters, Stack<MutableMap<String, CoreInstance>> resolvedMultiplicityParameters, VariableContext variableContext, MutableStack<CoreInstance> functionExpressionCallStack, Profiler profiler, InstantiationContext instantiationContext, ExecutionSupport executionSupport, ProcessorSupport processorSupport, FunctionExecutionInterpreted interpreted)
    {
        if (!Instance.instanceOf(function, M3Paths.Column, processorSupport))
        {
            return null;
        }
        // The receiver may arrive as the raw row OR wrapped in an
        // InstanceValue (the param-passing convention varies by call site).
        // Locate the row by trying both and picking the one whose
        // classifierGenericType.rawType is a RelationType (i.e., one of
        // our TDSTuple rows). This is also the receiver-scoped
        // discriminator: any other Column application (legend-engine's
        // RowContainer-backed rows) falls through to its own hook.
        CoreInstance receiver = params.get(0);
        CoreInstance row = pickTdsTupleRow(receiver, processorSupport);
        if (row == null)
        {
            CoreInstance unwrapped = receiver.getValueForMetaPropertyToOne(M3Properties.values);
            row = (unwrapped == null) ? null : pickTdsTupleRow(unwrapped, processorSupport);
        }
        if (row == null)
        {
            return null;
        }
        CoreInstance rawType = row.getValueForMetaPropertyToOne(M3Properties.classifierGenericType).getValueForMetaPropertyToOne(M3Properties.rawType);
        CoreInstance receiverRow = row;
        ListIterable<? extends CoreInstance> cols = rawType.getValueForMetaPropertyToMany(M3Properties.columns);
        String colName = function.getValueForMetaPropertyToOne(M3Properties.name).getName();
        int index = -1;
        for (int i = 0; i < cols.size(); i++)
        {
            if (colName.equals(cols.get(i).getValueForMetaPropertyToOne(M3Properties.name).getName()))
            {
                index = i;
                break;
            }
        }
        if (index < 0)
        {
            return null;
        }

        // Read holders[index].values → null when the inner List is empty.
        ListIterable<? extends CoreInstance> holders = receiverRow.getValueForMetaPropertyToMany(M3Properties.values);
        CoreInstance holder = holders.get(index);
        CoreInstance value = holder.getValueForMetaPropertyToOne(M3Properties.values);
        // Wrap as an InstanceValue (the convention `FunctionExecutionInterpreted`
        // expects from hook results). `null` => empty (multiplicity 0).
        if (value == null)
        {
            return ValueSpecificationBootstrap.wrapValueSpecification(
                    org.eclipse.collections.api.factory.Lists.mutable.empty(), false, processorSupport);
        }
        CoreInstance wrappedValue = ValueSpecificationBootstrap.wrapValueSpecification(value, false, processorSupport);

        // Extended-primitive columns (`SmallInt extends Integer`, …)
        // need their constraint(s) evaluated on every cell read. The
        // engine's `Cast` does this for `cast(@SmallInt)`; mirror that
        // here so `$row.col` honours the type's invariant — without
        // this, an out-of-range value reads back silently and
        // testSimpleWithCastMap (SmallInt = 7 vs `$this < 256`) would
        // pass when it should fail.
        CoreInstance columnType = function.getValueForMetaPropertyToOne(M3Properties.classifierGenericType);
        if (columnType != null)
        {
            CoreInstance columnGT = _Column.getColumnType((org.finos.legend.pure.m3.coreinstance.meta.pure.metamodel.relation.Column<?, ?>) function);
            if (columnGT != null)
            {
                CoreInstance rawColType = columnGT.getValueForMetaPropertyToOne(M3Properties.rawType);
                if (rawColType != null && Type.isExtendedPrimitiveType(rawColType, processorSupport))
                {
                    Cast.evaluateConstraints(
                            wrappedValue,
                            columnGT,
                            interpreted,
                            instantiationContext,
                            functionExpressionCallStack,
                            functionExpressionCallStack.peek().getSourceInformation(),
                            executionSupport,
                            processorSupport);
                }
            }
        }
        return wrappedValue;
    }

    private static CoreInstance pickTdsTupleRow(CoreInstance candidate, ProcessorSupport processorSupport)
    {
        CoreInstance cgt = candidate.getValueForMetaPropertyToOne(M3Properties.classifierGenericType);
        CoreInstance rawType = (cgt == null) ? null : cgt.getValueForMetaPropertyToOne(M3Properties.rawType);
        if (rawType != null && Instance.instanceOf(rawType, M3Paths.RelationType, processorSupport))
        {
            return candidate;
        }
        return null;
    }
}
