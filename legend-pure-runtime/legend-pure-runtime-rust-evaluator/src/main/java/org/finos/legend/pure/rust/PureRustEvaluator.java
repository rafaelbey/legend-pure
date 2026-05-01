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

import org.eclipse.collections.api.LazyIterable;
import org.eclipse.collections.api.factory.Maps;
import org.eclipse.collections.impl.factory.Lists;
import org.eclipse.collections.impl.utility.LazyIterate;

import java.io.Closeable;
import java.lang.ref.Cleaner;
import java.util.Map;


/**
 * A JNI-based evaluator for Pure using the Rust-based execution engine.
 * <p>
 * This class is <b>thread-safe</b>. Concurrent access to the evaluator or its
 * derived {@link PureRustInstance} objects is synchronized internally.
 * <p>
 * This evaluator manages off-heap memory in the Rust runtime. Explicitly calling {@link #close()}
 * will immediately free all associated native resources and invalidate any live {@link PureRustInstance}
 * references returned by this evaluator.
 * <p>
 * Explicitly closing the evaluator is optional; if the instance is garbage collected,
 * the {@link java.lang.ref.Cleaner} will automatically free the off-heap reference Rust manages.
 */
public class PureRustEvaluator implements Closeable
{
    private final static Cleaner cleaner = Cleaner.create();

    // Load the native library
    static {
        // We'll manage loading the appropriate JNI library for the OS
        System.loadLibrary("pure_rust_jni");
    }

    // Opaque pointer to the Rust-side compiler/runtime context
    private final long contextPointer;
    private final Cleaner.Cleanable cleanable;
    private final Map<Long, Cleaner.Cleanable> instanceCleanables = Maps.mutable.empty();
    private boolean closed;

    /**
     * Initialize the Rust evaluator context with the given source repository.
     */
    public PureRustEvaluator() {
        long context = nativeInitContext();
        this.contextPointer = context;
        this.cleanable = cleaner.register(this, () -> PureRustEvaluator.nativeFreeContext(context));
    }

    /**
     * Executes a Pure function by its fully qualified path.
     * <p>
     * The arguments can be primitive types (String, Boolean, Long, Integer, Double, Float),
     * {@link PureRustInstance} objects, or {@link Iterable} collections of these types.
     *
     * @param functionPath the fully qualified path of the function to execute (e.g., "meta::myFunction__String_1_")
     * @param args the arguments to pass to the function
     * @param <T> the expected return type
     * @return the result of the evaluation, automatically unwrapped from native types
     * @throws PureRustEvaluationException if the evaluator is closed or a native execution error occurs
     */
    public synchronized <T> T evaluate(String functionPath, Object... args)
    {
        if (this.closed)
        {
            throw new PureRustEvaluationException("Evaluator has been closed");
        }

        PureRustResult pureRustResult = nativeEvaluate(this.contextPointer, functionPath, wrapRustResults(args));
        //noinspection unchecked
        return (T) unwrapRustResult(pureRustResult);
    }

    private PureRustResult[] wrapRustResults(Object[] args)
    {
        PureRustResult[] pureRustResult = new PureRustResult[args.length];
        for (int i = 0; i < args.length; i++)
        {
            pureRustResult[i] = wrapRustResult(args[i]);
        }
        return pureRustResult;
    }

    private PureRustResult wrapRustResult(Object arg)
    {
        if (arg == null)
        {
            return new PureRustResult(this.contextPointer);
        }

        if (arg instanceof PureRustInstance)
        {
            return new PureRustResult((PureRustInstance) arg, this.contextPointer);
        }

        if (arg instanceof Iterable)
        {
            Iterable<?> iterable = (Iterable<?>) arg;
            LazyIterable<PureRustResult> results = LazyIterate.adapt(iterable).collect(this::wrapRustResult);
            return new PureRustResult(results.toArray(new PureRustResult[0]), this.contextPointer);
        }

        if (arg instanceof String)
        {
            return new PureRustResult((String) arg, this.contextPointer);
        }

        if (arg instanceof Boolean)
        {
            return new PureRustResult((Boolean) arg, this.contextPointer);
        }

        if (arg instanceof Long || arg instanceof Integer)
        {
            return new PureRustResult(((Number) arg).longValue(), this.contextPointer);
        }

        if (arg instanceof Double || arg instanceof Float)
        {
            return new PureRustResult(((Number) arg).doubleValue(), this.contextPointer);
        }

        throw new IllegalArgumentException("Unsupported argument type: " + arg.getClass());
    }

    private Object unwrapRustResult(PureRustResult pureRustResult)
    {
        switch (pureRustResult.getType())
        {
            case NULL:
                return null;
            case BOOLEAN:
                return pureRustResult.getAsBoolean();
            case STRING:
                return pureRustResult.getAsString();
            case INTEGER:
                return pureRustResult.getAsInteger();
            case FLOAT:
                return pureRustResult.getAsFloat();
            case INSTANCE_POINTER:
                long instancePointer = pureRustResult.getAsInstancePointer();
                PureRustInstance instance = new PureRustInstance(instancePointer, this);
                this.instanceCleanables.put(instancePointer, cleaner.register(instance, () -> nativeFreeInstance(this.contextPointer, instancePointer)));
                return instance;
            case ARRAY:
                return Lists.mutable.with(pureRustResult.getAsArray()).collect(this::unwrapRustResult);
            default:
                throw new RuntimeException("Unsupported type " + pureRustResult.getType());
        }
    }

    /**
     * Evaluates a property on a native instance.
     *
     * @param instance the instance to evaluate property on
     * @param propertyName the name of the property to evaluate
     * @param args additional arguments for the property evaluation (e.g., parameters for a qualified property)
     * @param <T> the expected return type
     * @return the result of the property evaluation
     * @throws PureRustEvaluationException if the evaluator is closed
     */
    protected synchronized <T> T evaluateProperty(PureRustInstance instance, String propertyName, Object... args)
    {
        if (this.closed)
        {
            throw new PureRustEvaluationException("Evaluator has been closed");
        }

        PureRustResult pureRustResult = nativeGetProperty(this.contextPointer, instance.instancePointer, propertyName, wrapRustResults(args));
        //noinspection unchecked
        return (T) unwrapRustResult(pureRustResult);
    }

    /**
     * Returns the fully qualified classifier path for the given instance.
     *
     * @param pureRustInstance the instance to query
     * @return the classifier path (e.g., "meta::pure::metamodel::type::Class")
     * @throws PureRustEvaluationException if the evaluator is closed
     */
    protected synchronized String getClassifier(PureRustInstance pureRustInstance)
    {
        if (this.closed)
        {
            throw new PureRustEvaluationException("Evaluator has been closed");
        }

        return nativeGetClassifier(this.contextPointer, pureRustInstance.instancePointer);
    }

    /**
     * Manually frees a native instance reference.
     * <p>
     * While this is called automatically by the GC, it can be invoked explicitly to
     * release native memory immediately.
     *
     * @param pureRustInstance the instance to free
     */
    protected synchronized void free(PureRustInstance pureRustInstance)
    {
        Cleaner.Cleanable cleanabe = this.instanceCleanables.remove(pureRustInstance.instancePointer);
        if (cleanabe != null)
        {
            cleanabe.clean();
        }
    }

    /**
     * Closes the evaluator and releases all associated native resources.
     * <p>
     * Once closed, any further calls to this evaluator or its derived instances
     * will throw a {@link PureRustEvaluationException}.
     */
    @Override
    public synchronized void close()
    {
        this.closed = true;
        this.instanceCleanables.values().forEach(Cleaner.Cleanable::clean);
        this.instanceCleanables.clear();
        this.cleanable.clean();
    }

    // --- JNI Native Methods ---
    private native static long nativeInitContext();
    private native static void nativeFreeContext(long contextPtr);
    private native static void nativeFreeInstance(long contextPtr, long instancePointer);
    private native static PureRustResult nativeEvaluate(long contextPtr, String functionPath, PureRustResult[] args);
    private native static PureRustResult nativeGetProperty(long contextPtr, long instancePtr, String propertyName, PureRustResult[] args);
    private native static String nativeGetClassifier(long contextPtr, long instancePtr);
}
