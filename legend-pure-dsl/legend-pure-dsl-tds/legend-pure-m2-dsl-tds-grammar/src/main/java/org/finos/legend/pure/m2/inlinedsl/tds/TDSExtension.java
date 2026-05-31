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

package org.finos.legend.pure.m2.inlinedsl.tds;

import io.deephaven.csv.CsvSpecs;
import io.deephaven.csv.parsers.DataType;
import io.deephaven.csv.parsers.Parsers;
import io.deephaven.csv.reading.CsvReader;
import io.deephaven.csv.sinks.SinkFactory;
import io.deephaven.csv.util.CsvReaderException;

import java.util.Arrays;

import org.eclipse.collections.api.RichIterable;
import org.eclipse.collections.api.block.function.Function;
import org.eclipse.collections.api.factory.Lists;
import org.eclipse.collections.api.list.ListIterable;
import org.eclipse.collections.api.list.MutableList;
import org.eclipse.collections.api.tuple.Pair;
import org.eclipse.collections.impl.tuple.Tuples;
import org.eclipse.collections.impl.utility.ListIterate;
import org.finos.legend.pure.m2.inlinedsl.tds.processor.TDSProcessor;
import org.finos.legend.pure.m2.inlinedsl.tds.unloader.TDSUnbind;
import org.finos.legend.pure.m2.inlinedsl.tds.validation.TDSVisibilityValidator;
import org.finos.legend.pure.m3.compiler.Context;
import org.finos.legend.pure.m3.compiler.postprocessing.processor.valuespecification.InstanceValueProcessor;
import org.finos.legend.pure.m3.compiler.validation.validator.GenericTypeValidator;
import org.finos.legend.pure.m3.coreinstance.CoreInstanceFactoryRegistry;
import org.finos.legend.pure.m3.coreinstance.TDSCoreInstanceFactoryRegistry;
import org.finos.legend.pure.m3.coreinstance.meta.pure.metamodel._import.ImportGroup;
import org.finos.legend.pure.m3.coreinstance.meta.pure.metamodel.function.FunctionAccessor;
import org.finos.legend.pure.m3.coreinstance.meta.pure.metamodel.multiplicity.Multiplicity;
import org.finos.legend.pure.m3.coreinstance.meta.pure.metamodel.relation.Column;
import org.finos.legend.pure.m3.coreinstance.meta.pure.metamodel.relation.RelationType;
import org.finos.legend.pure.m3.coreinstance.meta.pure.metamodel.relation.TDS;
import org.finos.legend.pure.m3.coreinstance.meta.pure.metamodel.type.Class;
import org.finos.legend.pure.m3.coreinstance.meta.pure.metamodel.type.Type;
import org.finos.legend.pure.m3.coreinstance.meta.pure.metamodel.type.generics.GenericType;
import org.finos.legend.pure.m3.coreinstance.meta.pure.metamodel.valuespecification.InstanceValue;
import org.finos.legend.pure.m3.navigation.M3Paths;
import org.finos.legend.pure.m3.navigation.M3Properties;
import org.finos.legend.pure.m3.navigation.M3ProcessorSupport;
import org.finos.legend.pure.m3.navigation.ProcessorSupport;
import org.finos.legend.pure.m3.navigation._package._Package;
import org.finos.legend.pure.m3.navigation.relation._Column;
import org.finos.legend.pure.m3.navigation.relation._RelationType;
import org.finos.legend.pure.m3.serialization.grammar.m3parser.antlr.M3AntlrParser;
import org.finos.legend.pure.m3.serialization.grammar.m3parser.antlr.M3Parser;
import org.finos.legend.pure.m3.serialization.grammar.m3parser.inlinedsl.InlineDSL;
import org.finos.legend.pure.m3.serialization.grammar.m3parser.inlinedsl.MilestoningDatesVarNamesExtractor;
import org.finos.legend.pure.m3.serialization.grammar.m3parser.inlinedsl.VisibilityValidator;
import org.finos.legend.pure.m3.tools.matcher.MatchRunner;
import org.finos.legend.pure.m4.ModelRepository;
import org.finos.legend.pure.m4.coreinstance.CoreInstance;
import org.finos.legend.pure.m4.coreinstance.SourceInformation;
import org.finos.legend.pure.m4.exception.PureCompilationException;
import org.finos.legend.pure.m4.serialization.grammar.antlr.AntlrSourceInformation;

import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

public class TDSExtension implements InlineDSL
{
    private static final VisibilityValidator VISIBILITY_VALIDATOR = new TDSVisibilityValidator();

    @Override
    public String getName()
    {
        return "TDS";
    }

    @Override
    public boolean match(String code)
    {
        return code.startsWith("TDS");
    }

    @Override
    public CoreInstance parse(String code, ImportGroup importId, String fileName, int columnOffset, int lineOffset, ModelRepository modelRepository, Context context)
    {
        String text = code.substring("TDS".length()).trim();
        Pair<String, GenericType> res = extractBodyAndType(text, header ->
                {
                    CoreInstance expression = new M3AntlrParser().parseExpression(true, "~[" + header + "]", fileName, columnOffset, lineOffset, importId, modelRepository, context);
                    return (GenericType) expression.getValueForMetaPropertyToMany("parametersValues").get(1).getValueForMetaPropertyToOne("genericType");
                }
        );
        return parse(res.getTwo(), res.getOne(), getSourceInfo(code, fileName, columnOffset, lineOffset), new M3ProcessorSupport(context, modelRepository));
    }

    @Override
    public RichIterable<MatchRunner> getValidators()
    {
        return Lists.mutable.empty();
    }

    @Override
    public RichIterable<MatchRunner> getProcessors()
    {
        return Lists.immutable.with(new TDSProcessor());
    }

    @Override
    public RichIterable<MatchRunner> getUnLoadWalkers()
    {
        return Lists.mutable.empty();
    }

    @Override
    public RichIterable<MatchRunner> getUnLoadUnbinders()
    {
        return Lists.immutable.with(new TDSUnbind());
    }

    @Override
    public RichIterable<CoreInstanceFactoryRegistry> getCoreInstanceFactoriesRegistry()
    {
        return Lists.immutable.with(TDSCoreInstanceFactoryRegistry.REGISTRY);
    }

    @Override
    public VisibilityValidator getVisibilityValidator()
    {
        return VISIBILITY_VALIDATOR;
    }

    @Override
    public MilestoningDatesVarNamesExtractor getMilestoningDatesVarNamesExtractor()
    {
        return null;
    }

    public static TDS<?> parse(String text, SourceInformation sourceInfo, ProcessorSupport processorSupport)
    {
        Pair<String, GenericType> res = extractBodyAndType(text, header ->
                {
                    AntlrSourceInformation sourceInformation = new AntlrSourceInformation(0, 0, "", true);
                    org.finos.legend.pure.m3.serialization.grammar.m3parser.antlr.M3Parser parser = M3AntlrParser.initAntlrParser(true, "~[" + header + "]", sourceInformation);

                    return (GenericType) processorSupport.type_wrapGenericType(_RelationType.build(ListIterate.collect(parser.columnBuilders().oneColSpec(), oneColSpec ->
                            {
                                String name = oneColSpec.columnName().getText().trim();
                                if (name.startsWith("'"))
                                {
                                    name = name.substring(1, name.length() - 1).trim();
                                }
                                Multiplicity multiplicity = oneColSpec.multiplicity() == null ? null : processMultiplicity(oneColSpec.multiplicity().multiplicityArgument(), processorSupport);
                                GenericType type = processType(oneColSpec.type(), processorSupport);
                                return _Column.getColumnInstance(name.trim(), false, type, multiplicity, sourceInfo, processorSupport);
                            }
                    ), null, processorSupport));
                }
        );
        return parse(res.getTwo(), res.getOne(), sourceInfo, processorSupport);
    }

    private static GenericType processType(M3Parser.TypeContext type, ProcessorSupport processorSupport)
    {
        GenericType target = (GenericType) processorSupport.newAnonymousCoreInstance(null, M3Paths.GenericType);
        if (type == null)
        {
            target._rawType(null);
        }
        else
        {
            CoreInstance _type = _Package.getByUserPath(type.qualifiedName().getText(), processorSupport);
            if (_type == null)
            {
                throw new PureCompilationException(type.qualifiedName().getText() + " not found!  (imports are not scan for TDS column type resolution)");
            }
            target._rawType((Type) _type)
                    ._typeVariableValues(type.typeVariableValues() == null ? Lists.mutable.empty() : ListIterate.collect(type.typeVariableValues().instanceLiteral(), x ->
                    {
                        InstanceValue res = (InstanceValue)processorSupport.newAnonymousCoreInstance(null, M3Paths.InstanceValue);
                        res._valuesAdd(Integer.valueOf(x.instanceLiteralToken().INTEGER().getText()));
                        InstanceValueProcessor.updateInstanceValue(res, processorSupport);
                        return res;
                    })
                    );
        }
        return target;
    }

    private static Multiplicity processMultiplicity(M3Parser.MultiplicityArgumentContext ctx, ProcessorSupport processorSupport)
    {
        if (ctx.identifier() == null)
        {
            if ((ctx.fromMultiplicity() == null || "1".equals(ctx.fromMultiplicity().getText())) && "1".equals(ctx.toMultiplicity().getText()))
            {
                return (Multiplicity) processorSupport.package_getByUserPath(M3Paths.PureOne);
            }
            else if (ctx.fromMultiplicity() != null && "0".equals(ctx.fromMultiplicity().getText()) && "1".equals(ctx.toMultiplicity().getText()))
            {
                return (Multiplicity) processorSupport.package_getByUserPath(M3Paths.ZeroOne);
            }
            else
            {
                throw new RuntimeException("Not supported yet");
            }
        }
        return null;
    }

    public static Pair<String, GenericType> extractBodyAndType(String text, Function<String, GenericType> res)
    {
        int newlineIdx = text.indexOf("\n");
        String header;
        String body;
        if (newlineIdx == -1)
        {
            // No data rows — the whole input is the header. Previous
            // implementation used `text.length() - 1` and silently
            // dropped the last character of a header-only TDS, which
            // surfaced as `Parser error … expected identifier found ']'`
            // when the snipped column list was wrapped in `~[…]`.
            header = text;
            body = "";
        }
        else
        {
            header = text.substring(0, newlineIdx);
            body = text.substring(newlineIdx + 1);
        }
        return Tuples.pair(body, res.apply(header));
    }

    public static TDS<?> parse(GenericType headerType, String body, SourceInformation sourceInfo, ProcessorSupport processorSupport)
    {
        RelationType<?> givenRelationType = ((RelationType<?>) headerType._rawType());

        String fullText = givenRelationType._columns().collect(FunctionAccessor::_name).makeString(", ") + "\n" + body;

        CsvReader.Result result;
        try
        {
            result = CsvReader.read(makePureCsvSpecs(), new ByteArrayInputStream(fullText.getBytes(StandardCharsets.UTF_8)), makePureSinkFactory());
        }
        catch (CsvReaderException e)
        {
            throw new PureCompilationException(sourceInfo, e.getCause().getMessage());
        }

        // Pre-scan the typed pass to infer per-column multiplicity: if
        // any cell in a column is empty / `null`, that column is
        // `[0..1]`; otherwise it is `[1]`. A column with no data rows at
        // all defaults to `[0..1]` (we can't promise a value). Java
        // parity with the Rust `infer_column` rule (the inferred mult is
        // only applied when the user didn't pin one explicitly via the
        // header `name:Type[mult]` annotation).
        CsvReader.ResultColumn[] typedColumns = result.columns();
        long inferRowCount = result.numRows();
        boolean[] hasMissing = new boolean[typedColumns.length];
        for (int colIdx = 0; colIdx < typedColumns.length; colIdx++)
        {
            Object data = typedColumns[colIdx].data();
            for (long r = 0; r < inferRowCount; r++)
            {
                boolean missing;
                if (data instanceof String[])
                {
                    missing = ((String[]) data)[(int) r] == null;
                }
                else if (data instanceof long[])
                {
                    // Deephaven CSV writes the `CSV_NULL_LONG` sentinel
                    // we configured in `makePureSinkFactory` for null
                    // cells in the primitive `long[]` sink. (Same
                    // pattern for the other primitive types below.)
                    missing = ((long[]) data)[(int) r] == CSV_NULL_LONG;
                }
                else if (data instanceof int[])
                {
                    missing = ((int[]) data)[(int) r] == CSV_NULL_INT;
                }
                else if (data instanceof byte[])
                {
                    missing = ((byte[]) data)[(int) r] == CSV_NULL_BYTE;
                }
                else if (data instanceof short[])
                {
                    missing = ((short[]) data)[(int) r] == Short.MIN_VALUE;
                }
                else if (data instanceof char[])
                {
                    missing = ((char[]) data)[(int) r] == CSV_NULL_CHAR;
                }
                else if (data instanceof double[])
                {
                    missing = ((double[]) data)[(int) r] == CSV_NULL_DOUBLE;
                }
                else if (data instanceof float[])
                {
                    missing = ((float[]) data)[(int) r] == CSV_NULL_FLOAT;
                }
                else if (data instanceof Object[])
                {
                    missing = ((Object[]) data)[(int) r] == null;
                }
                else
                {
                    missing = false;
                }
                if (missing)
                {
                    hasMissing[colIdx] = true;
                    break;
                }
            }
        }
        final boolean[] hasMissingFinal = hasMissing;

        Multiplicity defaultZeroOne = (Multiplicity) org.finos.legend.pure.m3.navigation.multiplicity.Multiplicity.newMultiplicity(0, 1, processorSupport);
        Multiplicity defaultPureOne = (Multiplicity) processorSupport.package_getByUserPath(M3Paths.PureOne);
        RelationType<?> relationType = _RelationType.build(ListIterate.zip(Arrays.asList(typedColumns), givenRelationType._columns()).collectWithIndex((c, idx) ->
        {
            GenericType columnType = _Column.getColumnType(c.getTwo());
            Multiplicity multiplicity = _Column.getColumnMultiplicity(c.getTwo());
            if (multiplicity == null)
            {
                // Header didn't pin a multiplicity — infer from the
                // data: any missing cell drops to `[0..1]`, otherwise
                // we promise `[1]`. Empty data → `[0..1]` (header-only
                // TDS can't claim `[1]`).
                multiplicity = (inferRowCount == 0L || hasMissingFinal[idx])
                        ? defaultZeroOne
                        : defaultPureOne;
            }
            if (columnType == null || columnType.getValueForMetaPropertyToOne("rawType") == null)
            {
                return _Column.getColumnInstance(c.getTwo()._name(), false, convertType(c.getOne().dataType()), multiplicity, sourceInfo, processorSupport);
            }
            else
            {
                return _Column.getColumnInstance(c.getTwo()._name(), false, (GenericType) org.finos.legend.pure.m3.navigation.generictype.GenericType.copyGenericType(columnType, sourceInfo, processorSupport), multiplicity, sourceInfo, processorSupport);
            }
        }), sourceInfo, processorSupport);

        Class<?> tdsType = (Class<?>) processorSupport.package_getByUserPath(M2TDSPaths.TDS);
        GenericType typeParam = ((GenericType) processorSupport.newAnonymousCoreInstance(sourceInfo, M3Paths.GenericType))._rawType(relationType);
        GenericType tdsGenericType = ((GenericType) processorSupport.newAnonymousCoreInstance(sourceInfo, M3Paths.GenericType))
                ._rawType(tdsType)
                ._typeArgumentsAdd(typeParam);
        GenericTypeValidator.validateGenericType(tdsGenericType, processorSupport);

        // Materialise typed rows — the source of truth. `csv` is now a
        // derived qualified property (see `tds.pure` / the `tdsToCsv`
        // native), so we store `rows : T[*]` instead of a `csv` slot.
        //
        // A second Deephaven pass with a string-only parser yields each
        // cell's original text (same splitting as the typed pass above,
        // so cell columns align by index with `relationType._columns()`).
        // Each non-null cell is routed to its column's inferred Pure
        // primitive type; empty / `null` cells become absent slots
        // (`[0..1]` semantics), mirroring the Rust runtime's row tuples.
        CsvReader.Result stringResult;
        try
        {
            stringResult = CsvReader.read(makePureStringSpecs(), new ByteArrayInputStream(fullText.getBytes(StandardCharsets.UTF_8)), makePureSinkFactory());
        }
        catch (CsvReaderException e)
        {
            throw new PureCompilationException(sourceInfo, e.getCause().getMessage());
        }

        MutableList<? extends Column<?, ?>> columns = relationType._columns().toList();
        CsvReader.ResultColumn[] cellColumns = stringResult.columns();
        // Deephaven sinks pre-allocate column arrays larger than the actual
        // filled rows (growth strategy); use the Result's `numRows()` for the
        // true row count rather than `array.length`.
        int rowCount = (int) stringResult.numRows();
        MutableList<CoreInstance> rows = Lists.mutable.empty();
        for (int r = 0; r < rowCount; r++)
        {
            // Build the N `List<Any>` cell-holders in column order. Inner
            // list of size 0 = null cell, size 1 = present cell.
            MutableList<CoreInstance> cellHolders = Lists.mutable.withInitialCapacity(columns.size());
            for (int c = 0; c < columns.size(); c++)
            {
                CoreInstance holder = processorSupport.newAnonymousCoreInstance(sourceInfo, M3Paths.List);
                String cell = ((String[]) cellColumns[c].data())[r];
                if (cell != null)
                {
                    CoreInstance value = makeCellValue(processorSupport, _Column.getColumnType(columns.get(c)), cell);
                    holder.setKeyValues(Lists.mutable.with("values"), Lists.mutable.with(value));
                }
                cellHolders.add(holder);
            }

            // The Java-backing class is TDSTuple (concrete + named, so the
            // compiled engine generates a Root_..._TDSTuple_Impl with a
            // fixed `_values` field — dodging the dynamic-property wall on
            // anonymous classifiers). We then OVERRIDE
            // `classifierGenericType` to the runtime RelationType so Pure
            // type checks see the row as an instance of T (satisfying
            // `rows : T[*]`). Java-level `instanceof Root_TDSTuple_Impl`
            // and Pure-level "instance of T" deliberately diverge — see
            // TDSTuple's comment in tds.pure.
            CoreInstance row = processorSupport.newAnonymousCoreInstance(sourceInfo, M2TDSPaths.TDSTuple);
            row.setKeyValues(Lists.mutable.with("values"), cellHolders);

            GenericType rowClassifierGT = ((GenericType) processorSupport.newAnonymousCoreInstance(sourceInfo, M3Paths.GenericType))
                    ._rawType((Type) relationType);
            row.setKeyValues(Lists.mutable.with(M3Properties.classifierGenericType), Lists.mutable.with(rowClassifierGT));

            rows.add(row);
        }

        TDS<?> tds = ((TDS<?>) processorSupport.newAnonymousCoreInstance(sourceInfo, M2TDSPaths.TDS))
                ._classifierGenericType(tdsGenericType);
        ((TDS) tds)._rows(rows);
        return tds;
    }

    // Route a cell's text to its column's inferred Pure primitive type,
    // producing a value CoreInstance whose classifier is that primitive
    // type and whose name carries the literal (the standard Pure
    // primitive representation; PrimitiveUtilities reads the value back
    // off the name). Mirrors the Rust runtime's `typed_cell_to_value`.
    // Created via ProcessorSupport so it works on the runtime native
    // path, which has no ModelRepository handle.
    private static CoreInstance makeCellValue(ProcessorSupport processorSupport, GenericType columnType, String cell)
    {
        Type rawType = columnType == null ? null : (Type) columnType._rawType();
        // Primitive value instances may not carry source information, so
        // these are created with a null SourceInformation.
        if (rawType == null)
        {
            return processorSupport.newCoreInstance(cell, M3Paths.String, null);
        }
        // Decimal cells keep their D/d suffix in source form; strip it so
        // the stored literal is a bare number (renderCell re-adds the D).
        String value = ("Decimal".equals(rawType.getName()) && (cell.endsWith("D") || cell.endsWith("d"))) ? cell.substring(0, cell.length() - 1) : cell;
        return processorSupport.newCoreInstance(value, rawType, null);
    }

    private static SourceInformation getSourceInfo(String text, String fileName, int columnOffset, int lineOffset)
    {
        int endLine = lineOffset;
        int endLineIndex = 0;
        Matcher matcher = Pattern.compile("\\R").matcher(text);
        while (matcher.find())
        {
            endLine++;
            endLineIndex = matcher.end();
        }

        int endColumn = (endLine == lineOffset) ? (text.length() + columnOffset - 1) : (text.length() - endLineIndex);
        return new SourceInformation(fileName, lineOffset, columnOffset, endLine, endColumn);
    }

    private static String convertType(DataType dataType)
    {
        switch (dataType)
        {
            case BOOLEAN_AS_BYTE:
            {
                return M3Paths.Boolean;
            }
            case BYTE:
            case SHORT:
            case INT:
            case LONG:
            {
                return M3Paths.Integer;
            }
            case DATETIME_AS_LONG:
            {
                return M3Paths.Date;
            }
            case FLOAT:
            case DOUBLE:
            {
                return M3Paths.Float;
            }
            case STRING:
            case CHAR:
            {
                return M3Paths.String;
            }
            case TIMESTAMP_AS_LONG:
            case CUSTOM:
            {
                throw new RuntimeException("Not possible");
            }
            default:
            {
                // TODO is this correct?
                return "";
            }
        }
    }

    public static CsvSpecs makePureCsvSpecs()
    {
        return CsvSpecs.builder().nullValueLiterals(Arrays.asList("", "null")).build();
    }

    // Same null-literal handling as makePureCsvSpecs, but forces every
    // column to parse as a String so we recover each cell's original
    // text (for row materialisation) rather than a width-narrowed
    // primitive array. Empty / `null` cells surface as `null` entries.
    public static CsvSpecs makePureStringSpecs()
    {
        return CsvSpecs.builder().nullValueLiterals(Arrays.asList("", "null")).parsers(Parsers.STRINGS).build();
    }

    // Null sentinels Deephaven uses in `makePureSinkFactory`'s primitive
    // arrays. The values here are the *exact* tokens written into the
    // typed-result columns when a cell parses as null. Used by the
    // per-column missing-cell pre-scan in `parse(GenericType, String, …)`
    // to infer column multiplicity (`[0..1]` if any cell missing,
    // otherwise `[1]`).
    private static final int CSV_NULL_INT = 2_147_483_647;
    private static final long CSV_NULL_LONG = 9_223_372_036_854_775_783L;
    private static final float CSV_NULL_FLOAT = Float.NEGATIVE_INFINITY;
    private static final double CSV_NULL_DOUBLE = Double.NEGATIVE_INFINITY;
    private static final byte CSV_NULL_BYTE = Byte.MIN_VALUE;
    private static final char CSV_NULL_CHAR = Character.MIN_VALUE;

    public static SinkFactory makePureSinkFactory()
    {
        return SinkFactory.arrays(
                null,
                null,
                CSV_NULL_INT, //largest prime for 32 signed numbers
                CSV_NULL_LONG, //largest prime for 64 signed numbers
                CSV_NULL_FLOAT,
                CSV_NULL_DOUBLE,
                CSV_NULL_BYTE,
                CSV_NULL_CHAR,
                null,
                Long.MIN_VALUE,
                Long.MIN_VALUE);
    }

    // Render a TDS's canonical CSV (header line + one line per row) from
    // its `rows` + column `RelationType`. Backs the derived `TDS.csv()`
    // qualified property via the `tdsToCsv` native (compiled +
    // interpreted). Canonical format (mirrors the Rust runtime's
    // `render_csv_from_columns_and_rows`):
    //
    // - Header: `name:Type[mult]` per column. Type tag always included;
    //   multiplicity bracket appended only when the column's mult differs
    //   from the column-default `[0..1]`.
    // - Separator: `,` (no space) for both header and rows.
    // - Cells: empty for absent values (both `null` literal and bare
    //   empty input round-trip through the same canonical form), no
    //   surrounding quotes on strings — see renderCell.
    public static String renderCsv(TDS<?> tds)
    {
        RelationType<?> relationType = (RelationType<?>) tds._classifierGenericType()._typeArguments().getFirst()._rawType();
        MutableList<? extends Column<?, ?>> columns = relationType._columns().toList();
        StringBuilder out = new StringBuilder();
        out.append(columns.collect(TDSExtension::renderColumnHeader).makeString(","));
        for (Object rowObj : tds._rows())
        {
            CoreInstance row = (CoreInstance) rowObj;
            out.append("\n");
            ListIterable<? extends CoreInstance> cellHolders = row.getValueForMetaPropertyToMany("values");
            MutableList<String> cells = Lists.mutable.withInitialCapacity(columns.size());
            for (int i = 0; i < columns.size(); i++)
            {
                // Each holder is a `List<Any>` instance: 0 elements = null,
                // 1 element = present. `getValueForMetaPropertyToOne` returns
                // null when the inner list is empty.
                CoreInstance holder = cellHolders.get(i);
                CoreInstance value = holder.getValueForMetaPropertyToOne("values");
                cells.add(renderCell(value, _Column.getColumnType(columns.get(i))));
            }
            out.append(cells.makeString(","));
        }
        return out.toString();
    }

    // Render a column header as `name[:Type][[mult]]` per the canonical
    // CSV format. Type tag always emitted (falls back to `String` when
    // the column's classifierGenericType doesn't resolve a type element);
    // multiplicity bracket omitted for the column-default `[0..1]`.
    private static String renderColumnHeader(Column<?, ?> column)
    {
        StringBuilder out = new StringBuilder(column._name());
        GenericType columnType = _Column.getColumnType(column);
        Type rawType = columnType == null ? null : (Type) columnType._rawType();
        String typeName = rawType == null ? "String" : rawType.getName();
        out.append(':').append(typeName);
        String multStr = renderColumnMultiplicity(column);
        if (multStr != null)
        {
            out.append(multStr);
        }
        return out.toString();
    }

    // Bracket suffix `[1]`, `[*]`, `[1..*]`, `[m..n]` from a Column's
    // multiplicity, or `null` for the column-default `[0..1]` (elided).
    // Reads the multiplicity from the column's
    // classifierGenericType.multiplicityArguments[0]; falls back to
    // null when no multiplicity argument is present.
    private static String renderColumnMultiplicity(Column<?, ?> column)
    {
        ListIterable<? extends CoreInstance> multArgs = column._classifierGenericType().getValueForMetaPropertyToMany("multiplicityArguments");
        if (multArgs.isEmpty())
        {
            return null;
        }
        CoreInstance mult = multArgs.get(0);
        if (mult == null)
        {
            return null;
        }
        Long lower = readBound(mult, "lowerBound");
        Long upper = readBound(mult, "upperBound");
        if (lower == null)
        {
            return null;
        }
        long lo = lower.longValue();
        if (upper == null)
        {
            // unbounded upper
            return lo == 0L ? "[*]" : lo == 1L ? "[1..*]" : "[" + lo + "..*]";
        }
        long up = upper.longValue();
        if (lo == 0L && up == 1L)
        {
            return null; // column default, elided
        }
        if (lo == up)
        {
            return "[" + lo + "]";
        }
        return "[" + lo + ".." + up + "]";
    }

    // Read `<bound>.value` from a Multiplicity heap object (`lowerBound`
    // / `upperBound` each carry a numeric `value`). Returns `null` for
    // an absent bound or a non-Integer value slot.
    private static Long readBound(CoreInstance mult, String boundName)
    {
        CoreInstance bound = mult.getValueForMetaPropertyToOne(boundName);
        if (bound == null)
        {
            return null;
        }
        CoreInstance v = bound.getValueForMetaPropertyToOne("value");
        if (v == null)
        {
            return null;
        }
        try
        {
            return Long.parseLong(v.getName());
        }
        catch (NumberFormatException nfe)
        {
            return null;
        }
    }

    // Render a single cell to its canonical CSV form. Mirrors the Rust
    // runtime's `render_cell`: null/absent → empty (no quotes); Integer
    // / Boolean / dates verbatim; Float forced to carry a decimal point;
    // Decimal suffixed with D; String verbatim (no surrounding quotes).
    // The canonical form drops the legacy single-quote wrap and the
    // `''` null sentinel in favour of bare-empty fields, matching what
    // `stringToTDS` accepts back through its bare-empty / `null` literal
    // round-trip.
    private static String renderCell(CoreInstance value, GenericType columnType)
    {
        if (value == null)
        {
            return "";
        }
        Type rawType = columnType == null ? null : (Type) columnType._rawType();
        String typeName = rawType == null ? "String" : rawType.getName();
        switch (typeName)
        {
            case "Integer":
            case "Boolean":
                return value.getName();
            case "Float":
            {
                String s = value.getName();
                return (s.indexOf('.') < 0 && s.indexOf('e') < 0 && s.indexOf('E') < 0) ? s + ".0" : s;
            }
            case "Decimal":
                return value.getName() + "D";
            case "StrictDate":
            case "Date":
            case "DateTime":
                return value.getName();
            case "String":
            default:
                return value.getName();
        }
    }
}
