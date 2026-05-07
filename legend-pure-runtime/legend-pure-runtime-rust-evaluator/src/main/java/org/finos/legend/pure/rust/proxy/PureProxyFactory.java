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

import java.lang.reflect.ParameterizedType;
import java.lang.reflect.Proxy;
import java.lang.reflect.Type;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Wraps native results coming back from the JNI evaluator into the
 * generated Java types — and unwraps the proxies users hand back in.
 *
 * <p>Generated interfaces register themselves here at class-load time
 * via the {@code $REGISTERED} sentinel field they each carry. Subtype
 * dispatch consults this registry: when the runtime classifier of an
 * incoming {@link PureRustInstance} matches a generated interface, the
 * factory proxies as that interface (most-specific match), so callers
 * can {@code instanceof}-check the actual subtype rather than only the
 * declared one.
 *
 * <p>The registry is normally warmed by the static initializer of the
 * generated facade class (e.g. {@code PureFunctions}), which reads the
 * {@code $REGISTERED} field of every reachable generated interface.
 * Callers that bypass the facade and invoke
 * {@link PureRustInstance#getProperty(String, Object...)} directly
 * should pre-touch any interfaces they need so the registry is
 * populated before {@code wrap(...)} runs.
 */
public final class PureProxyFactory
{
    private static final Map<String, Class<?>> CLASSIFIER_REGISTRY = new ConcurrentHashMap<>();

    private PureProxyFactory() {}

    /**
     * Register a generated interface for a Pure classifier FQN.
     * Called from each generated interface's {@code $REGISTERED}
     * initializer.
     *
     * @param classifierFqn the Pure-side classifier (e.g.
     *                      {@code "user_test::Person"})
     * @param iface         the generated Java interface
     */
    public static void register(String classifierFqn, Class<?> iface)
    {
        CLASSIFIER_REGISTRY.put(classifierFqn, iface);
    }

    /**
     * Wrap a native result from the JNI evaluator into the declared Java
     * type. {@code declaredType} is typically a {@code Class<?>} for raw
     * types, or {@code null} when the caller could not preserve the
     * generic. The factory falls back to the runtime classifier when the
     * declared type would otherwise be ambiguous (e.g. {@code Object},
     * {@code null}).
     *
     * @param raw          the value returned by
     *                     {@link PureRustEvaluator#evaluate(String, Object...)}
     *                     or
     *                     {@link PureRustInstance#getProperty(String, Object...)}
     * @param declaredType the static return type at the call site, or
     *                     {@code null} when unknown
     * @param eval         the evaluator that owns any nested
     *                     {@link PureRustInstance}s
     * @param <T>          the declared static return type
     * @return the wrapped value, possibly a Java reflection proxy
     */
    @SuppressWarnings("unchecked")
    public static <T> T wrap(Object raw, Type declaredType, PureRustEvaluator eval)
    {
        boolean optionalDeclared = declaredType instanceof ParameterizedType
                && Optional.class.equals(((ParameterizedType) declaredType).getRawType());

        if (raw == null)
        {
            return optionalDeclared ? (T) Optional.empty() : null;
        }

        // Optional<X> declared on the return type — wrap.
        if (optionalDeclared)
        {
            Type elt = ((ParameterizedType) declaredType).getActualTypeArguments()[0];
            return (T) Optional.ofNullable(wrap(raw, elt, eval));
        }

        // Iterable<X>: wrap each element with the element type as the
        // declared type. Materialise to a List so the result is safely
        // re-iterable on the Java side.
        if (raw instanceof Iterable && !(raw instanceof PureRustInstance))
        {
            Iterable<?> iterable = (Iterable<?>) raw;
            Type eltType = declaredType instanceof ParameterizedType
                    ? ((ParameterizedType) declaredType).getActualTypeArguments()[0]
                    : null;
            List<Object> out = new ArrayList<>();
            for (Object element : iterable)
            {
                out.add(wrap(element, eltType, eval));
            }
            return (T) out;
        }

        // PureRustInstance: pick the interface to proxy as.
        if (raw instanceof PureRustInstance)
        {
            PureRustInstance instance = (PureRustInstance) raw;
            Class<?> declaredIface = asInterfaceClass(declaredType);
            Class<?> targetIface = pickInterface(instance, declaredIface);
            if (targetIface == null)
            {
                // No generated interface — return the raw instance so
                // callers retain the JNI handle.
                return (T) instance;
            }
            return (T) Proxy.newProxyInstance(
                    targetIface.getClassLoader(),
                    new Class<?>[] {targetIface},
                    new PureInvocationHandler(instance, eval, targetIface));
        }

        // Pass-through for primitives, Strings, etc.
        return (T) raw;
    }

    /**
     * Inverse of {@link #wrap}: turns proxies back into their underlying
     * {@link PureRustInstance}, walks {@link Iterable} and
     * {@link Optional} arguments element-wise, and otherwise passes
     * primitives through unchanged.
     *
     * @param value the user-supplied argument (possibly a proxy or a
     *              collection of proxies)
     * @return a value safe to hand to
     *         {@link PureRustEvaluator#evaluate(String, Object...)} or
     *         {@link PureRustInstance#getProperty(String, Object...)}
     */
    public static Object unwrap(Object value)
    {
        if (value == null)
        {
            return null;
        }
        if (value instanceof Optional)
        {
            return ((Optional<?>) value).map(PureProxyFactory::unwrap).orElse(null);
        }
        if (value instanceof PureRegistered)
        {
            return new PureRustInstanceHandle(((PureRegistered) value).$instancePointer());
        }
        if (value instanceof Iterable && !(value instanceof PureRustInstance))
        {
            List<Object> out = new ArrayList<>();
            for (Object element : (Iterable<?>) value)
            {
                out.add(unwrap(element));
            }
            return out;
        }
        return value;
    }

    /**
     * Return the most-specific generated interface registered for the
     * runtime classifier of {@code instance}, falling back to the
     * declared interface when the classifier is unregistered.
     */
    private static Class<?> pickInterface(PureRustInstance instance, Class<?> declaredIface)
    {
        String classifier;
        try
        {
            classifier = instance.getClassifier();
        }
        catch (RuntimeException ignored)
        {
            classifier = null;
        }
        if (classifier != null)
        {
            Class<?> registered = CLASSIFIER_REGISTRY.get(classifier);
            if (registered != null
                    && (declaredIface == null || declaredIface.isAssignableFrom(registered)))
            {
                return registered;
            }
        }
        if (declaredIface != null && PureRegistered.class.isAssignableFrom(declaredIface))
        {
            return declaredIface;
        }
        return null;
    }

    private static Class<?> asInterfaceClass(Type t)
    {
        if (t instanceof Class<?>)
        {
            Class<?> c = (Class<?>) t;
            return c.isInterface() ? c : null;
        }
        if (t instanceof ParameterizedType)
        {
            Type raw = ((ParameterizedType) t).getRawType();
            if (raw instanceof Class<?>)
            {
                Class<?> c = (Class<?>) raw;
                return c.isInterface() ? c : null;
            }
        }
        return null;
    }

    /**
     * Lightweight handle replacement we hand back to the JNI layer when
     * unwrapping a proxy. The Rust side reads the {@code instancePointer}
     * field via reflection (mirrors what the existing
     * {@link PureRustInstance} marshalling does); concrete subtype is
     * preserved for {@code instanceof} checks in the JNI conversion
     * layer.
     */
    public static final class PureRustInstanceHandle extends PureRustInstance
    {
        private PureRustInstanceHandle(long pointer)
        {
            super(pointer, null);
        }
    }
}
