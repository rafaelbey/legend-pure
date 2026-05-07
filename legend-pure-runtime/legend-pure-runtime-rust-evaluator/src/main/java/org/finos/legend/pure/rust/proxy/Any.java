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

import org.finos.legend.pure.rust.PureRustInstance;

/**
 * The universal supertype every generated Pure-class interface extends.
 * <p>
 * Mirrors Pure's {@code meta::pure::metamodel::type::Any} — every
 * Pure value is at least an {@code Any}, so the Java proxy lattice
 * makes the same guarantee. {@link PureProxyFactory#wrap} returns at
 * least an {@code Any} proxy whenever it has a {@link PureRustInstance}
 * to wrap, even when the runtime classifier isn't registered for any
 * more-specific generated interface. That gives callers a typed
 * fallback they can {@code instanceof}-narrow or drop down on via
 * {@link #$rustInstance()} to call dynamic
 * {@link PureRustInstance#getProperty(String, Object...)}.
 * <p>
 * <strong>Hand-written, not generated.</strong> The codegen recognises
 * {@code meta::pure::metamodel::type::Any} and substitutes references
 * to it with this interface; it is never emitted as Java source.
 */
public interface Any extends PureRegistered
{
    /**
     * The underlying native handle backing this proxy.
     * <p>
     * Useful for falling back to dynamic property access
     * ({@link PureRustInstance#getProperty(String, Object...)}),
     * for {@link PureRustInstance#getClassifier()}, or for handing
     * the value back to {@link org.finos.legend.pure.rust.PureRustEvaluator#evaluate}
     * as an opaque argument.
     */
    PureRustInstance $rustInstance();
}
