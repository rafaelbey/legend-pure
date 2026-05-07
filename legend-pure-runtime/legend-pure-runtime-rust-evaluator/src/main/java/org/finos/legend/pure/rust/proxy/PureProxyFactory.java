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

import java.lang.reflect.Method;
import java.lang.reflect.ParameterizedType;
import java.lang.reflect.Proxy;
import java.lang.reflect.Type;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.IdentityHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
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
    private static final Map<Class<?>, String> INTERFACE_TO_CLASSIFIER = new ConcurrentHashMap<>();

    static
    {
        // The universal supertype is always available; the codegen
        // substitutes references to `meta::pure::metamodel::type::Any`
        // with this hand-written interface and never emits Any.java.
        register("meta::pure::metamodel::type::Any", Any.class);
    }

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
        INTERFACE_TO_CLASSIFIER.put(iface, classifierFqn);
    }

    /**
     * Materialise a user implementation of a generated interface as a
     * real heap object, and return a proxy backed by the new instance.
     * <p>
     * The factory walks {@code iface}'s 0-arg property methods,
     * invokes each on {@code userImpl} (default methods on
     * {@code [0..1]}/{@code [*]} properties return empty unless the
     * user overrode them), recursively materialises any nested
     * {@link PureRegistered} values that aren't already proxies, and
     * unwraps proxy arguments to handle pointers via
     * {@link #unwrap(Object)}.
     * <p>
     * Qualified properties (methods with parameters) are skipped —
     * their semantics is computed and can't be stored as a value.
     *
     * @param userImpl an instance of {@code iface} the caller wrote
     *                 by hand
     * @param iface    the generated interface declaring the schema
     * @param eval     the evaluator that owns the resulting heap
     *                 pointer
     * @param <T>      the interface type
     * @return a proxy backed by the freshly-allocated heap object
     */
    @SuppressWarnings("unchecked")
    public static <T extends PureRegistered> T create(T userImpl, Class<T> iface, PureRustEvaluator eval)
    {
        // Top-level entry: cycle-detection cache is fresh per call.
        IdentityHashMap<PureRegistered, PureRustInstance> visited = new IdentityHashMap<>();
        PureRustInstance instance = createInstance(userImpl, iface, eval, visited);
        return (T) Proxy.newProxyInstance(
                iface.getClassLoader(),
                new Class<?>[] {iface},
                new PureInvocationHandler(instance, eval, iface));
    }

    /**
     * Materialise a user impl as a {@link PureRustInstance} (raw heap
     * pointer, no proxy wrap). The recursive path uses this so nested
     * results land on the JNI side as {@code PureRustInstance} — which
     * {@link PureRustEvaluator#create} knows how to marshal — rather
     * than as a {@link Proxy} that {@code wrapRustResult} doesn't
     * recognise.
     */
    private static PureRustInstance createInstance(
            PureRegistered userImpl,
            Class<?> iface,
            PureRustEvaluator eval,
            IdentityHashMap<PureRegistered, PureRustInstance> visited)
    {
        if (userImpl == null) throw new IllegalArgumentException("userImpl is null");
        if (iface == null) throw new IllegalArgumentException("iface is null");

        // Cycle / sharing guard: if we've already materialised this
        // exact user object (identity, not equals), reuse the existing
        // heap instance so reference structure is preserved and
        // self-references don't blow the stack.
        PureRustInstance existing = visited.get(userImpl);
        if (existing != null) return existing;

        // If the input is itself a proxy, its handle is already on the
        // heap — return that, no re-materialisation needed.
        if (Proxy.isProxyClass(userImpl.getClass()))
        {
            // PureInvocationHandler stores the underlying instance;
            // unwrap to recover it.
            Object unwrapped = unwrap(userImpl);
            if (unwrapped instanceof PureRustInstance)
            {
                return (PureRustInstance) unwrapped;
            }
        }

        String classifierFqn = lookupClassifier(iface);

        List<Method> propertyMethods = collectStorableProperties(iface);
        String[] names = new String[propertyMethods.size()];
        Object[] values = new Object[propertyMethods.size()];

        for (int i = 0; i < propertyMethods.size(); i++)
        {
            Method m = propertyMethods.get(i);
            names[i] = pureNameOf(m);
            Object raw;
            try
            {
                raw = m.invoke(userImpl);
            }
            catch (ReflectiveOperationException e)
            {
                throw new RuntimeException(
                        "failed to invoke `" + m.getName() + "` on user impl of " + iface.getName(),
                        e.getCause() != null ? e.getCause() : e);
            }
            values[i] = materialiseValue(raw, eval, visited);
        }

        PureRustInstance instance = eval.create(classifierFqn, names, values);
        visited.put(userImpl, instance);
        return instance;
    }

    private static String lookupClassifier(Class<?> iface)
    {
        String classifierFqn = INTERFACE_TO_CLASSIFIER.get(iface);
        if (classifierFqn == null)
        {
            // Force interface initialisation so the registry populates
            // (`$REGISTERED` is a side-effect-init field; reading any
            // static-final on the interface triggers it).
            try { Class.forName(iface.getName(), true, iface.getClassLoader()); }
            catch (ClassNotFoundException ignored) { }
            classifierFqn = INTERFACE_TO_CLASSIFIER.get(iface);
        }
        if (classifierFqn == null)
        {
            throw new IllegalStateException(
                    "no classifier registered for " + iface.getName()
                            + " — was the interface generated by legend java-bindings?");
        }
        return classifierFqn;
    }

    /**
     * Recursively realise a value returned from a user impl method.
     * Returns Java-side values that {@link PureRustEvaluator#create}
     * already knows how to marshal: primitives/strings pass through,
     * {@link PureRegistered} impls become {@link PureRustInstance}
     * via {@link #createInstance}, lists become lists of marshalled
     * elements.
     */
    private static Object materialiseValue(
            Object raw,
            PureRustEvaluator eval,
            IdentityHashMap<PureRegistered, PureRustInstance> visited)
    {
        if (raw == null) return null;
        if (raw instanceof Optional)
        {
            return ((Optional<?>) raw).map(v -> materialiseValue(v, eval, visited)).orElse(null);
        }
        if (raw instanceof PureRegistered)
        {
            PureRegistered reg = (PureRegistered) raw;
            // Already on the heap — pull the underlying instance so
            // wrapRustResult sees a PureRustInstance, not the proxy.
            if (Proxy.isProxyClass(raw.getClass()))
            {
                return new PureRustInstanceHandle(reg.$instancePointer());
            }
            // Pick the most-specific generated interface this user
            // class implements so the recursive create finds the right
            // classifier. NOTE: the picker walks declared interfaces
            // only — a user class with a deeper hierarchy may need
            // refinement (advisor follow-up).
            Class<?> targetIface = pickGeneratedInterface(raw.getClass());
            if (targetIface == null)
            {
                throw new IllegalStateException(
                        raw.getClass().getName()
                                + " implements PureRegistered but no generated interface is "
                                + "registered for it");
            }
            return createInstance(reg, targetIface, eval, visited);
        }
        if (raw instanceof Iterable && !(raw instanceof PureRustInstance))
        {
            List<Object> out = new ArrayList<>();
            for (Object element : (Iterable<?>) raw)
            {
                out.add(materialiseValue(element, eval, visited));
            }
            return out;
        }
        return raw;
    }

    private static Class<?> pickGeneratedInterface(Class<?> userClass)
    {
        Class<?> best = null;
        for (Class<?> c = userClass; c != null; c = c.getSuperclass())
        {
            for (Class<?> i : c.getInterfaces())
            {
                if (PureRegistered.class.isAssignableFrom(i)
                        && INTERFACE_TO_CLASSIFIER.containsKey(i)
                        && (best == null || best.isAssignableFrom(i)))
                {
                    best = i;
                }
            }
        }
        return best;
    }

    /**
     * Return the property methods on {@code iface} (and its supertypes)
     * that we know how to store: zero-arg, declared on a generated
     * interface, not a sentinel, not a Java {@link Object} method.
     * Qualified properties (methods with parameters) are filtered out
     * — we can only persist storage-shaped properties.
     */
    private static List<Method> collectStorableProperties(Class<?> iface)
    {
        List<Method> out = new ArrayList<>();
        Set<String> seenNames = new java.util.HashSet<>();
        for (Method m : iface.getMethods())
        {
            if (m.getParameterCount() != 0) continue;
            if (java.lang.reflect.Modifier.isStatic(m.getModifiers())) continue;
            if (m.getDeclaringClass() == Object.class) continue;
            if (m.getDeclaringClass() == PureRegistered.class) continue;
            String name = m.getName();
            if (name.equals("$instancePointer") || name.equals("$evaluator")) continue;
            if (name.equals("__register")) continue;
            if (!seenNames.add(name)) continue; // dedup overrides
            out.add(m);
        }
        return out;
    }

    /**
     * Strip the trailing {@code _} added by the codegen for properties
     * whose Pure name collides with a Java keyword (e.g.
     * {@code class_} → {@code class}).
     */
    private static String pureNameOf(Method m)
    {
        String n = m.getName();
        if (n.length() > 1 && n.endsWith("_")) return n.substring(0, n.length() - 1);
        return n;
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
        // pickInterface always returns at least Any.class, so every
        // heap object hands back a typed proxy that the caller can
        // instanceof-narrow or drop down on via $rustInstance().
        if (raw instanceof PureRustInstance)
        {
            PureRustInstance instance = (PureRustInstance) raw;
            Class<?> declaredIface = asInterfaceClass(declaredType);
            Class<?> targetIface = pickInterface(instance, declaredIface);
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
     * Pick the interface to proxy a {@link PureRustInstance} as, in
     * decreasing specificity:
     * <ol>
     *   <li>The most-specific generated interface registered for the
     *       runtime classifier of {@code instance}, when that interface
     *       is assignable to {@code declaredIface}.</li>
     *   <li>The declared interface itself, when it's a generated
     *       (i.e. {@link PureRegistered}-extending) interface.</li>
     *   <li>{@link Any} — the universal fallback. Every Pure heap
     *       object is at least an Any, so callers always get a typed
     *       proxy with a {@link Any#$rustInstance()} drop-down to
     *       dynamic dispatch.</li>
     * </ol>
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
        return Any.class;
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
