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

package org.finos.legend.pure.rust.bootstrap;

import org.finos.legend.pure.rust.bindings.PureBindings;

/**
 * Marker class that triggers Pure-bindings codegen for the M3
 * metamodel manifest at
 * {@code src/main/resources/pure-bindings/m3-bindings.jpure}.
 *
 * <p>The manifest path is a classpath-relative resource. Its
 * {@code @pkg:} directive declares the Java root package
 * ({@code org.finos.legend.pure.rust.generated}); no annotation
 * fallback is needed.
 *
 * <p>Generated sources land in
 * {@code target/generated-sources/annotations/org/finos/legend/pure/rust/generated/}
 * via the {@link PureBindings} annotation processor. They're picked up
 * automatically by javac in the same compile that runs the AP — no
 * extra source-root registration needed.
 */
@PureBindings(bindingsFile = "pure-bindings/m3-bindings.jpure")
public final class M3Bootstrap
{
    private M3Bootstrap()
    {
    }
}
