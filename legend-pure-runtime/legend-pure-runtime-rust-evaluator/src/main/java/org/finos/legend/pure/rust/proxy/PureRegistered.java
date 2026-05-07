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

/**
 * Marker supertype for every interface emitted by
 * {@code legend java-bindings}. Two purposes:
 * <ul>
 *   <li>Lets {@link PureProxyFactory#wrap(Object, java.lang.reflect.Type, PureRustEvaluator)}
 *       cheaply distinguish generated interfaces from arbitrary user
 *       interfaces with one {@code instanceof} check.</li>
 *   <li>Declares the two sentinel accessors the runtime invocation
 *       handler relies on to recover the underlying handle and the
 *       evaluator that owns it. Generated interfaces inherit these for
 *       free.</li>
 * </ul>
 *
 * Method names use {@code $} so they don't collide with any Pure
 * property name.
 */
public interface PureRegistered
{
    /**
     * The native instance pointer that backs this proxy.
     *
     * @return the i64 handle (as a {@code long}) corresponding to the
     *         underlying {@link org.finos.legend.pure.rust.PureRustInstance}.
     */
    long $instancePointer();

    /**
     * The {@link PureRustEvaluator} that owns the wrapped instance.
     */
    PureRustEvaluator $evaluator();
}
