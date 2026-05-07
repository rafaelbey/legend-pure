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

import java.io.Closeable;
import java.lang.ref.Cleaner;

/**
 * A handle to a native Pure instance managed by the Rust execution engine.
 * <p>
 * This object is a proxy to an off-heap resource. It is <b>not</b> thread-safe
 * and must be used in conjunction with the {@link PureRustEvaluator} that created it.
 * <p>
 * Closing this instance will release the native reference. If not closed explicitly,
 * the native reference will be released when this object is garbage collected.
 */
public class PureRustInstance implements Closeable
{
    protected final long instancePointer;
    private final PureRustEvaluator owner;

    protected PureRustInstance(long instancePointer, PureRustEvaluator owner)
    {
        this.instancePointer = instancePointer;
        this.owner = owner;
    }

    /**
     * Returns the fully qualified classifier path of this instance.
     *
     * @return the classifier path
     */
    public String getClassifier()
    {
        return this.owner.getClassifier(this);
    }

    /**
     * The underlying native instance handle.
     * <p>
     * Exposed publicly so the proxy support code in
     * {@code org.finos.legend.pure.rust.proxy} can read the pointer
     * without joining this package.
     */
    public long instancePointer()
    {
        return this.instancePointer;
    }

    /**
     * Retrieves a property value from this instance.
     *
     * @param propertyName the name of the property
     * @param args optional arguments for the property (e.g., for qualified properties)
     * @param <T> the expected return type
     * @return the property value
     */
    public <T> T getProperty(String propertyName, Object... args)
    {
        return this.owner.evaluateProperty(this, propertyName, args);
    }

    /**
     * Manually releases the native reference for this instance.
     * <p>
     * After calling this, the instance should no longer be used.
     */
    @Override
    public void close()
    {
        this.owner.free(this);
    }
}
