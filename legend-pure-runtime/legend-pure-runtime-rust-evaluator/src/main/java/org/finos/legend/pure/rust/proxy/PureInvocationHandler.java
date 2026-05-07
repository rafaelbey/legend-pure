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

package org.finos.legend.pure.rust.proxy;

import org.finos.legend.pure.rust.PureRustEvaluator;
import org.finos.legend.pure.rust.PureRustInstance;

import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;

/**
 * Backs every proxy created by {@link PureProxyFactory}. Routes:
 * <ul>
 *   <li>{@code equals} / {@code hashCode} / {@code toString} to identity
 *       on the wrapped {@link PureRustInstance}'s pointer;</li>
 *   <li>the {@code $instancePointer()} / {@code $evaluator()} sentinels
 *       to the captured fields, without touching the evaluator;</li>
 *   <li>any other call (zero-arg or qualified-property) to
 *       {@link PureRustInstance#getProperty(String, Object...)}, with
 *       arguments unwrapped first and the result re-wrapped into the
 *       declared return type — including {@code Optional<X>} and
 *       {@code Iterable<X>}.</li>
 * </ul>
 *
 * The handler reads {@link Method#getGenericReturnType()} (not
 * {@code getReturnType()}) so the parameterised element type of an
 * {@code Iterable<X>} survives erasure and recursive proxying still
 * picks the right interface.
 */
public final class PureInvocationHandler implements InvocationHandler
{
    private final PureRustInstance instance;
    private final PureRustEvaluator evaluator;
    private final Class<?> iface;

    /**
     * @param instance  the JNI-side handle this proxy stands in for
     * @param evaluator the evaluator that owns {@code instance}
     * @param iface     the generated interface this proxy implements
     */
    public PureInvocationHandler(
            PureRustInstance instance,
            PureRustEvaluator evaluator,
            Class<?> iface)
    {
        this.instance = instance;
        this.evaluator = evaluator;
        this.iface = iface;
    }

    @Override
    public Object invoke(Object proxy, Method method, Object[] args) throws Throwable
    {
        String name = method.getName();
        Class<?> declaringClass = method.getDeclaringClass();

        // Object methods reach the handler — keep identity coherent.
        if (declaringClass == Object.class)
        {
            switch (name)
            {
                case "equals":
                    return args != null && args.length == 1 && equalsByPointer(args[0]);
                case "hashCode":
                    return Long.hashCode(instance.instancePointer());
                case "toString":
                    return iface.getSimpleName() + "@" + instance.instancePointer();
                default:
                    return method.invoke(this, args);
            }
        }

        // Sentinel methods on the proxy supertype lattice. Routed by
        // declaring class so they don't accidentally collide with a
        // Pure property of the same name.
        if (declaringClass == PureRegistered.class)
        {
            if ("$instancePointer".equals(name))
            {
                return instance.instancePointer();
            }
            if ("$evaluator".equals(name))
            {
                return evaluator;
            }
        }
        if (declaringClass == Any.class)
        {
            if ("$rustInstance".equals(name))
            {
                return instance;
            }
        }

        // Generated interfaces ship `default` bodies for `[0..1]` and
        // `[*]` properties (Optional.empty / emptyList) so user
        // implementations of the interface don't have to override every
        // field. For a *proxy* those defaults are irrelevant — the
        // runtime heap is the source of truth, so we always go native
        // here. The default body only matters for user `implements`
        // classes that inherit it.
        Object[] unwrapped = unwrapArgs(args);
        // Use the public `getProperty(...)` entry point on PureRustInstance
        // — `evaluator.evaluateProperty(...)` is intentionally protected.
        Object propertyResult = instance.getProperty(javaToPureName(name), unwrapped);
        return PureProxyFactory.wrap(
                propertyResult, method.getGenericReturnType(), evaluator);
    }

    private boolean equalsByPointer(Object other)
    {
        if (other == null) return false;
        // Two proxies are equal iff their underlying handles match.
        if (other instanceof PureRegistered)
        {
            return ((PureRegistered) other).$instancePointer() == instance.instancePointer();
        }
        if (other instanceof PureRustInstance)
        {
            return ((PureRustInstance) other).instancePointer() == instance.instancePointer();
        }
        return false;
    }

    private static Object[] unwrapArgs(Object[] args)
    {
        if (args == null) return new Object[0];
        Object[] out = new Object[args.length];
        for (int i = 0; i < args.length; i++)
        {
            out[i] = PureProxyFactory.unwrap(args[i]);
        }
        return out;
    }

    /**
     * Strip the trailing {@code _} added by the codegen for properties
     * whose Pure name collides with a Java keyword (e.g.
     * {@code class_} → {@code class}).
     */
    private static String javaToPureName(String javaMethodName)
    {
        if (javaMethodName.length() > 1 && javaMethodName.endsWith("_"))
        {
            return javaMethodName.substring(0, javaMethodName.length() - 1);
        }
        return javaMethodName;
    }
}
