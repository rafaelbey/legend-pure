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

import junit.extensions.TestSetup;
import junit.framework.Test;
import junit.framework.TestSuite;
import org.eclipse.collections.api.factory.Lists;
import org.eclipse.collections.api.list.MutableList;
import org.eclipse.collections.api.map.MutableMap;
import org.finos.legend.pure.m3.exception.PureAssertFailException;
import org.finos.legend.pure.m3.execution.ExecutionSupport;
import org.finos.legend.pure.m3.execution.FunctionExecution;
import org.finos.legend.pure.m3.execution.test.PureTestBuilder;
import org.finos.legend.pure.m3.execution.test.TestCollection;
import org.finos.legend.pure.m3.navigation.PackageableElement.PackageableElement;
import org.finos.legend.pure.m3.navigation.ProcessorSupport;
import org.finos.legend.pure.m3.navigation.ValueSpecificationBootstrap;
import org.finos.legend.pure.m3.navigation._package._Package;
import org.finos.legend.pure.m3.pct.reports.config.PCTReportConfiguration;
import org.finos.legend.pure.m3.pct.reports.config.exclusion.ExclusionSpecification;
import org.finos.legend.pure.m3.pct.reports.model.Adapter;
import org.finos.legend.pure.m3.pct.shared.PCTTools;
import org.finos.legend.pure.m3.pct.shared.model.ReportScope;
import org.finos.legend.pure.m4.coreinstance.CoreInstance;
import org.finos.legend.pure.rust.PureRustEvaluator;

import static org.finos.legend.pure.m3.pct.shared.PCTTools.isPCTTest;
import static org.junit.Assert.fail;

/**
 * Builds the JUnit 3 suite that runs a report scope's PCT tests through
 * the Rust evaluator. The executor is the interpreted engine's
 * ({@code PureTestBuilderInterpreted.getTestExecutor}) with the
 * {@link FunctionExecution} type widened and the outcome recorded per
 * test, so the report and the expected-failures manifest come from the
 * run itself.
 */
public final class PureTestBuilderRust
{
    /**
     * The platform name the Rust evaluator reports under.
     */
    public static final String PLATFORM = "rust-interpreted";

    private PureTestBuilderRust()
    {
    }

    /**
     * Discover the scope's PCT tests in the shared platform graph, validate
     * the manifest against them, and wrap the suite so one evaluator serves
     * every test and the collector flushes when the suite ends.
     *
     * @param reportScope      package and source-path prefix of the tests
     * @param expectedFailures the committed manifest, exploded by the harness
     * @param adapter          the adapter function every test receives as argument 0
     * @param collector        where outcomes are recorded
     * @return the suite
     */
    public static Test buildPCTTestSuite(ReportScope reportScope, MutableList<ExclusionSpecification> expectedFailures, Adapter adapter, PCTRunCollector collector)
    {
        ProcessorSupport processorSupport = RustPlatformGraph.processorSupport();
        MutableMap<String, String> explodedExpectedFailures = PCTReportConfiguration.explodeExpectedFailures(expectedFailures, processorSupport);
        TestCollection testCollection = TestCollection.buildPCTTestCollection(reportScope._package, reportScope.filePath, processorSupport);
        TestCollection.validateExclusions(testCollection, explodedExpectedFailures);

        PureRustEvaluator[] holder = new PureRustEvaluator[1];
        PureTestBuilder.F2<CoreInstance, MutableList<Object>, Object> testExecutor = getTestExecutor(() -> holder[0], processorSupport, explodedExpectedFailures, adapter.function, collector);
        TestSuite inner = new TestSuite();
        inner.addTest(PureTestBuilder.buildSuite(testCollection, testExecutor, new ExecutionSupport()
        {
        }));
        return new TestSetup(inner)
        {
            @Override
            protected void setUp()
            {
                holder[0] = RustPlatformGraph.newEvaluator();
            }

            @Override
            protected void tearDown()
            {
                try
                {
                    collector.flush();
                    long live = holder[0].liveHandleCount();
                    if (live != 0)
                    {
                        fail("Rust handles still alive after the suite: " + live);
                    }
                }
                finally
                {
                    holder[0].close();
                }
            }
        };
    }

    private static PureTestBuilder.F2<CoreInstance, MutableList<Object>, Object> getTestExecutor(java.util.function.Supplier<FunctionExecution> functionExecution, ProcessorSupport processorSupport, MutableMap<String, String> expectedFailures, String pctExecutor, PCTRunCollector collector)
    {
        return (a, b) ->
        {
            String testPath = PackageableElement.getUserPathForPackageableElement(a, "::");
            MutableList<CoreInstance> params = Lists.mutable.empty();
            if (isPCTTest(a, processorSupport))
            {
                CoreInstance adapter = _Package.getByUserPath(pctExecutor, processorSupport);
                if (adapter == null)
                {
                    throw new RuntimeException("The adapter " + pctExecutor + " can't be found in the graph");
                }
                params.add(ValueSpecificationBootstrap.wrapValueSpecification(adapter, true, processorSupport));
            }
            String message = expectedFailures.get(testPath);
            try
            {
                Object ret = functionExecution.get().start(a, params);
                collector.recordPass(testPath, message != null);
                if (message != null)
                {
                    PCTTools.displayExpectedErrorFailMessage(message, a, pctExecutor);
                    fail("Test was expected to fail with \"" + message + "\" but now passes — run with -Dlegend.pct.rust.rebase=true to update the manifest.");
                }
                return ret;
            }
            catch (Exception e)
            {
                collector.recordFailure(testPath, e);
                if ((message != null) && PCTTools.getMessageFromError(e).contains(message))
                {
                    return null;
                }
                PCTTools.displayErrorMessage(message, a, pctExecutor, processorSupport, e);
                if (e.getCause() instanceof PureAssertFailException)
                {
                    fail(e.getCause().getMessage());
                }
                if (e.getCause() != null)
                {
                    throw e.getCause();
                }
                throw e;
            }
        };
    }
}
