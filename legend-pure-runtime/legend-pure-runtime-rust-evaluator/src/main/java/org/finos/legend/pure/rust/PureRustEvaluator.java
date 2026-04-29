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
import org.eclipse.collections.impl.factory.Lists;
import org.eclipse.collections.impl.utility.LazyIterate;

import java.lang.ref.Cleaner;

public class PureRustEvaluator implements AutoCloseable
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

    /**
     * Initialize the Rust evaluator context with the given source repository.
     */
    public PureRustEvaluator() {
        long context = nativeInitContext();
        this.contextPointer = context;
        this.cleanable = cleaner.register(this, () -> PureRustEvaluator.nativeFreeContext(context));
    }

    /**
     * Execute a Pure function by its fully qualified path.
     * @param functionPath The full path (e.g., "meta::myFunction__String_1_")
     * @param args The arguments formatted as an array of RustResult variants
     * @return The evaluation result, structured as a RustResult
     */
    public <T> T evaluate(String functionPath, Object... args)
    {
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
                return new PureRustInstance(instancePointer, this);
            case ARRAY:
                return Lists.mutable.with(pureRustResult.getAsArray()).collect(this::unwrapRustResult);
            default:
                throw new RuntimeException("Unsupported type " + pureRustResult.getType());
        }
    }

    /**
     * Utility to evaluate a property directly given a complex pointer.
     */
    protected <T> T evaluateProperty(long instancePointer, String propertyName, Object... args)
    {
        PureRustResult pureRustResult = nativeGetProperty(this.contextPointer, instancePointer, propertyName, wrapRustResults(args));
        //noinspection unchecked
        return (T) unwrapRustResult(pureRustResult);
    }

    protected String getClassifier(PureRustInstance pureRustInstance)
    {
        return nativeGetClassifier(this.contextPointer, pureRustInstance.instancePointer);
    }

    @Override
    public void close()
    {
        this.cleanable.clean();
    }

    // --- JNI Native Methods ---
    private native static long nativeInitContext();
    private native static void nativeFreeContext(long contextPtr);

    private native static PureRustResult nativeEvaluate(long contextPtr, String functionPath, PureRustResult[] args);

    private native static PureRustResult nativeGetProperty(long contextPtr, long instancePtr, String propertyName, PureRustResult[] args);
    private native static String nativeGetClassifier(long contextPtr, long instancePtr);
}
