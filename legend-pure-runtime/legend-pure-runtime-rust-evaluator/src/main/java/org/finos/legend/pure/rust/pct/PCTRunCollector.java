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

package org.finos.legend.pure.rust.pct;

import com.fasterxml.jackson.databind.ObjectMapper;
import org.eclipse.collections.api.factory.Lists;
import org.eclipse.collections.api.factory.Maps;
import org.eclipse.collections.api.list.MutableList;
import org.eclipse.collections.api.map.MutableMap;
import org.finos.legend.pure.m3.pct.shared.PCTTools;
import org.finos.legend.pure.m3.pct.shared.model.PCTManifest;
import org.finos.legend.pure.m3.pct.shared.model.ReportScope;
import org.finos.legend.pure.rust.PureRustEvaluationException;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Outcomes of one report scope's run: the pass count, failures by kind, the
 * ranked list of functions the Rust side could not find, and the
 * expected-failures manifest regenerated from what actually happened.
 */
public final class PCTRunCollector
{
    /**
     * System property naming the report directory (set by surefire).
     */
    public static final String REPORT_DIR_PROPERTY = "legend.pct.rust.reportDir";
    /**
     * System property naming the committed manifest directory (set by surefire).
     */
    public static final String MANIFEST_DIR_PROPERTY = "legend.pct.rust.manifestDir";
    /**
     * When {@code true}, the regenerated manifest also overwrites the committed one.
     */
    public static final String REBASE_PROPERTY = "legend.pct.rust.rebase";

    private static final String FUNCTION_NOT_FOUND_PREFIX = "FunctionNotFound: ";

    private final ReportScope scope;
    private final String adapter;
    private final MutableMap<String, Outcome> outcomes = Maps.mutable.empty();

    /**
     * @param scope   the report scope
     * @param adapter the adapter function path, recorded in the manifest
     */
    public PCTRunCollector(ReportScope scope, String adapter)
    {
        this.scope = scope;
        this.adapter = adapter;
    }

    /**
     * @return the manifest resource path for a scope, relative to the test classpath root
     */
    public static String manifestResource(ReportScope scope)
    {
        return "pct-rust/pct_" + scope.module + "_native.json";
    }

    void recordPass(String testPath, boolean wasExpectedToFail)
    {
        this.outcomes.put(testPath, new Outcome(wasExpectedToFail ? "UNEXPECTED_PASS" : "PASS", null, null));
    }

    void recordFailure(String testPath, Throwable e)
    {
        String kind = (e instanceof PureRustEvaluationException) ? ((PureRustEvaluationException) e).getKind().name() : "OTHER";
        this.outcomes.put(testPath, new Outcome("FAIL", kind, PCTTools.getMessageFromError(e)));
    }

    /**
     * Print the pass rate, write the report and the regenerated manifest.
     */
    public void flush()
    {
        int total = this.outcomes.size();
        int passed = (int) this.outcomes.valuesView().count(o -> "PASS".equals(o.status));
        MutableMap<String, Integer> byKind = Maps.mutable.empty();
        MutableMap<String, Integer> notFound = Maps.mutable.empty();
        for (Outcome o : this.outcomes.valuesView())
        {
            if ("FAIL".equals(o.status))
            {
                byKind.updateValue(o.kind, () -> 0, n -> n + 1);
                if ((o.message != null) && o.message.startsWith(FUNCTION_NOT_FOUND_PREFIX))
                {
                    notFound.updateValue(o.message.substring(FUNCTION_NOT_FOUND_PREFIX.length()), () -> 0, n -> n + 1);
                }
            }
        }
        MutableList<String> ranked = notFound.keysView().toSortedList(Comparator.comparing((String k) -> -notFound.get(k)).thenComparing(k -> k));

        System.out.println(PureTestBuilderRust.PLATFORM + " " + this.scope.module + ": " + passed + " / " + total);
        ranked.take(20).forEach(name -> System.out.println("  FunctionNotFound " + notFound.get(name) + " x " + name));

        Map<String, Object> report = new LinkedHashMap<>();
        report.put("platform", PureTestBuilderRust.PLATFORM);
        Map<String, Object> scopeMap = new LinkedHashMap<>();
        scopeMap.put("module", this.scope.module);
        scopeMap.put("package", this.scope._package);
        scopeMap.put("filePath", this.scope.filePath);
        report.put("scope", scopeMap);
        report.put("adapter", this.adapter);
        report.put("total", total);
        report.put("passed", passed);
        report.put("failedByKind", new java.util.TreeMap<>(byKind));
        List<Map<String, Object>> notFoundList = new ArrayList<>();
        ranked.forEach(name ->
        {
            Map<String, Object> entry = new LinkedHashMap<>();
            entry.put("name", name);
            entry.put("count", notFound.get(name));
            notFoundList.add(entry);
        });
        report.put("functionNotFound", notFoundList);
        List<Map<String, Object>> tests = new ArrayList<>();
        this.outcomes.keysView().toSortedList().forEach(test ->
        {
            Outcome o = this.outcomes.get(test);
            Map<String, Object> entry = new LinkedHashMap<>();
            entry.put("test", test);
            entry.put("status", o.status);
            entry.put("kind", o.kind);
            entry.put("message", o.message);
            tests.add(entry);
        });
        report.put("tests", tests);

        PCTManifest manifest = new PCTManifest();
        manifest.adapter = this.adapter;
        manifest.exclusions = new ArrayList<>();
        this.outcomes.keysView().toSortedList().forEach(test ->
        {
            Outcome o = this.outcomes.get(test);
            if ("FAIL".equals(o.status))
            {
                PCTManifest.PCTManifestExclusion exclusion = new PCTManifest.PCTManifestExclusion();
                exclusion.test = test;
                exclusion.expectedError = o.message;
                manifest.exclusions.add(exclusion);
            }
        });

        ObjectMapper mapper = new ObjectMapper();
        try
        {
            Path reportDir = Paths.get(System.getProperty(REPORT_DIR_PROPERTY, "target/pct-rust"));
            Files.createDirectories(reportDir);
            mapper.writerWithDefaultPrettyPrinter().writeValue(reportDir.resolve(this.scope.module + ".json").toFile(), report);
            String manifestName = "pct_" + this.scope.module + "_native.json";
            mapper.writerWithDefaultPrettyPrinter().writeValue(reportDir.resolve(manifestName).toFile(), manifest);
            if (Boolean.getBoolean(REBASE_PROPERTY))
            {
                Path manifestDir = Paths.get(System.getProperty(MANIFEST_DIR_PROPERTY, "src/test/resources/pct-rust"));
                Files.createDirectories(manifestDir);
                mapper.writerWithDefaultPrettyPrinter().writeValue(manifestDir.resolve(manifestName).toFile(), manifest);
                System.out.println("  rebased " + manifestDir.resolve(manifestName));
            }
        }
        catch (IOException e)
        {
            throw new RuntimeException(e);
        }
    }

    private static final class Outcome
    {
        private final String status;
        private final String kind;
        private final String message;

        private Outcome(String status, String kind, String message)
        {
            this.status = status;
            this.kind = kind;
            this.message = message;
        }
    }
}
