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

/**
 * Placeholder for v2 — function-typed parameter support.
 * <p>
 * The current code generator (v1) refuses to emit wrappers for Pure
 * functions whose signatures take {@code Function<{...}>} parameters.
 * In v2 this interface will be the Java-side functional interface that
 * marshals a Java lambda into a Pure callable through a JNI callback
 * path. It is intentionally empty today so generated code can reference
 * the type without forward-compatibility break.
 */
public interface PureLambda
{
}
