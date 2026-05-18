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

package org.finos.legend.pure.rust;

import org.eclipse.collections.api.factory.Lists;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.Assertions;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

public class TestRustEvaluation
{
    private static PureRustEvaluator pureRustEvaluator;

    @BeforeAll
    static void setUp()
    {
        pureRustEvaluator = new PureRustEvaluator();
    }

    @AfterAll
    static void tearDown()
    {
        if (pureRustEvaluator != null)
        {
            pureRustEvaluator.close();
            PureRustEvaluationException excp = Assertions.assertThrows(PureRustEvaluationException.class, () -> pureRustEvaluator.evaluate(""));
            Assertions.assertEquals("Evaluator has been closed", excp.getMessage());
        }
    }

    @Test
    void testSimpleEvaluation()
    {
        long result = pureRustEvaluator.evaluate("meta::pure::functions::math::plus_Integer_MANY__Integer_1_", Lists.fixedSize.of(1, 4));
        Assertions.assertEquals(5, result);
    }

    @Test
    void tesEvaluationThrows()
    {
        // should throw - missing parameters
        Assertions.assertThrows(PureRustEvaluationException.class, () -> pureRustEvaluator.evaluate("meta::pure::functions::math::plus_Integer_MANY__Integer_1_"));

        // should throw - does not exist
        Assertions.assertThrows(PureRustEvaluationException.class, () -> pureRustEvaluator.evaluate("meta::pure::functions::math::plus__Any_1_"));
    }

    @Test
    void testInstanceEvaluation()
    {
        //equivalent to `123->genericType().rawType->toString();`

        // getting instance reference
        PureRustInstance result = pureRustEvaluator.evaluate("meta::pure::functions::meta::genericType_Any_MANY__GenericType_1_", 123);
        Assertions.assertEquals("meta::pure::metamodel::type::generics::GenericType", result.getClassifier());
        // eval property on instance
        PureRustInstance rawType = result.getProperty("rawType");
        //eval function with instance reference
        String rawTypeToString = pureRustEvaluator.evaluate("meta::pure::functions::string::toString_Any_1__String_1_", rawType);
        Assertions.assertEquals("Integer", rawTypeToString);

        // throws error when property does not exist.
        PureRustEvaluationException excp = Assertions.assertThrows(PureRustEvaluationException.class, () -> result.getProperty("notExistent"));
        Assertions.assertEquals("Property 'notExistent' not found", excp.getMessage(), "Message was:" + excp.getMessage());
    }

    @Test
    void testInstanceEvaluationReferenceCleanup()
    {
        PureRustInstance result = pureRustEvaluator.evaluate("meta::pure::functions::meta::genericType_Any_MANY__GenericType_1_", 123);
        // force the "GC"
        result.close();
        Assertions.assertThrows(PureRustEvaluationException.class, () -> result.getProperty("rawType"));
    }

    @Test
    void testEvaluationReferenceCleanup()
    {
        PureRustInstance result = pureRustEvaluator.evaluate("meta::pure::functions::meta::genericType_Any_MANY__GenericType_1_", 123);
        // force the "GC"
        result.close();
        PureRustEvaluationException excp = Assertions.assertThrows(PureRustEvaluationException.class, () -> result.getProperty("rawType"));
        Assertions.assertTrue(excp.getMessage().startsWith("Stale or unknown JNI handle:"), "Message was:" + excp.getMessage());
    }

    /**
     * Function-not-found is an FFI-boundary "execution" error: the Rust side
     * synthesises a {@code PureException} with kind {@code EXECUTION_ERROR}
     * and an empty call stack. The structured-throw helper must carry the
     * kind through to the Java exception's {@code getKind()} field.
     */
    @Test
    void testEvaluationExceptionCarriesKindField()
    {
        PureRustEvaluationException excp = Assertions.assertThrows(
                PureRustEvaluationException.class,
                () -> pureRustEvaluator.evaluate("meta::pure::functions::math::plus__Any_1_"));
        Assertions.assertEquals(PureRustEvaluationException.Kind.EXECUTION_ERROR, excp.getKind(),
                "kind was: " + excp.getKind());
        // Constraint-violation-only fields stay null for execution errors.
        Assertions.assertNull(excp.getConstraintId(), "constraintId should be null for execution error");
        Assertions.assertNull(excp.getConstraintKind(), "constraintKind should be null for execution error");
        Assertions.assertNull(excp.getOwnerFqn(), "ownerFqn should be null for execution error");
    }

    /**
     * End-to-end test for the structured-throw path on a constraint
     * violation. Triggers
     * {@code ^LA_ChildInheritsAgeConstraint(age=-1, name='X')} via the
     * Java-facing helper {@code LA_triggerInheritedConstraintViolation},
     * asserts the {@code PureRustEvaluationException} carries the
     * structured fields (kind = CONSTRAINT_VIOLATION, constraintId =
     * "ageNonNeg", constraintKind = CLASS, ownerFqn ending in
     * "LA_ParentWithAgeConstraint"). The legacy {@code getMessage()}
     * keeps the canonical violation-message shape for backwards
     * compatibility.
     */
    @Test
    void testConstraintViolationCarriesStructuredFields()
    {
        PureRustEvaluationException excp = Assertions.assertThrows(
                PureRustEvaluationException.class,
                () -> pureRustEvaluator.evaluate(
                        "meta::pure::functions::lang::tests::new::LA_triggerInheritedConstraintViolation"));
        Assertions.assertEquals(PureRustEvaluationException.Kind.CONSTRAINT_VIOLATION, excp.getKind(),
                "kind was: " + excp.getKind());
        Assertions.assertEquals("ageNonNeg", excp.getConstraintId(),
                "constraintId was: " + excp.getConstraintId());
        Assertions.assertEquals(PureRustEvaluationException.ConstraintKind.CLASS, excp.getConstraintKind(),
                "constraintKind was: " + excp.getConstraintKind());
        Assertions.assertNotNull(excp.getOwnerFqn(), "ownerFqn should be set");
        Assertions.assertTrue(excp.getOwnerFqn().endsWith("LA_ParentWithAgeConstraint"),
                "ownerFqn was: " + excp.getOwnerFqn());
        // Legacy contract: getMessage() carries the canonical violation
        // message string (used by assertError tests on the Pure side).
        Assertions.assertTrue(excp.getMessage().contains("Constraint :[ageNonNeg]"),
                "message was: " + excp.getMessage());
        Assertions.assertTrue(excp.getMessage().contains("age must be non-negative"),
                "message was: " + excp.getMessage());
    }
}
