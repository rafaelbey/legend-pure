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
}
