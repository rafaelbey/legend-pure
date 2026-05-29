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

package org.finos.legend.pure.runtime.java.extension.dsl.tds.compiled;

import java.math.BigDecimal;
import java.math.BigInteger;

import org.eclipse.collections.api.list.ListIterable;
import org.finos.legend.pure.m4.coreinstance.CoreInstance;
import org.finos.legend.pure.m4.coreinstance.primitive.date.DateFunctions;

/**
 * Compiled-engine helper invoked by the codegen produced by
 * {@code TDSExtensionCompiled.getExtraFunctionGeneration}.
 *
 * `$row.colName` on a {@code TDSTuple}-backed row compiles to
 * {@code (returnType) TDSTupleSupport.cellAt((CoreInstance)row, <index>)},
 * where {@code <index>} is the column's position in the receiver's
 * RelationType columns (resolved at codegen time).
 *
 * Each TDSTuple's {@code values} property holds N {@code List<Any>}
 * holders (one per column). An inner list of size 0 means the cell is
 * null; size 1 means the cell is present. Cell values are stored as
 * primitive {@code CoreInstance}s (e.g. {@code ValCoreInstance}); we
 * unbox them here to the Java boxed type the codegen cast expects
 * ({@link Long} for Integer, {@link Double} for Float, {@link String}
 * for String, etc.), matching how legend-engine's
 * {@code RelationNativeImplementation} produces column values from its
 * native {@code TestTDSCompiled} backing.
 */
public class TDSTupleSupport
{
    private TDSTupleSupport()
    {
    }

    /**
     * Read the i-th cell of a TDSTuple row and convert it to the Java boxed
     * type the compiled engine expects for the column's Pure primitive type.
     *
     * The {@code typeName} arg is the column's Pure raw-type name (e.g.
     * "Integer", "Float", "String"), embedded at codegen time by the
     * {@code TDSExtensionCompiled} hook — we do NOT call
     * {@code value.getClassifier()}, which a compiled-mode
     * {@code ValCoreInstance} actively forbids
     * ({@link AbstractCompiledCoreInstance#getClassifier} throws
     * {@link UnsupportedOperationException}).
     */
    public static Object cellAt(CoreInstance row, int index, String typeName)
    {
        ListIterable<? extends CoreInstance> holders = row.getValueForMetaPropertyToMany("values");
        if (index < 0 || index >= holders.size())
        {
            return null;
        }
        CoreInstance holder = holders.get(index);
        // Inner List<Any>.values: 0 = null cell, 1 = present.
        CoreInstance value = holder.getValueForMetaPropertyToOne("values");
        if (value == null)
        {
            return null;
        }
        switch (typeName)
        {
            case "Integer":
                return parseInteger(value.getName());
            case "Float":
                return Double.valueOf(value.getName());
            case "Decimal":
                return new BigDecimal(value.getName());
            case "Boolean":
                return Boolean.valueOf(value.getName());
            case "StrictDate":
            case "Date":
            case "DateTime":
                return DateFunctions.parsePureDate(value.getName());
            case "String":
            default:
                return value.getName();
        }
    }

    private static Number parseInteger(String literal)
    {
        try
        {
            return Long.valueOf(literal);
        }
        catch (NumberFormatException e)
        {
            // Values outside long range; the compiled engine accepts
            // BigInteger as a Number for Integer-typed columns.
            return new BigInteger(literal);
        }
    }
}
