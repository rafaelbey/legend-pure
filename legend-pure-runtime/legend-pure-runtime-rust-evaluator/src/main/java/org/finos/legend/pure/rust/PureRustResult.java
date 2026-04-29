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

import java.util.List;

class PureRustResult {
    public enum Type {
        NULL,
        BOOLEAN,
        INTEGER,
        FLOAT,
        STRING,
        INSTANCE_POINTER,
        ARRAY
    }

    private final Type type;
    private final Object value;
    private final long contextPointer;

    // Internal constructor used by JNI
    private PureRustResult(Type type, Object value, long contextPointer) {
        this.type = type;
        this.value = value;
        this.contextPointer = contextPointer;
    }

    PureRustResult(String value, long contextPointer) {
        this(Type.STRING, value, contextPointer);
    }

    PureRustResult(boolean value, long contextPointer) {
        this(Type.BOOLEAN, value, contextPointer);
    }

    PureRustResult(long value, long contextPointer) {
        this(Type.INTEGER, value, contextPointer);
    }

    PureRustResult(double value, long contextPointer) {
        this(Type.FLOAT, value, contextPointer);
    }

    PureRustResult(long contextPointer) {
        this(Type.NULL, null, contextPointer);
    }

    PureRustResult(PureRustResult[] value, long contextPointer) {
        this(Type.ARRAY, value, contextPointer);
    }

    PureRustResult(PureRustInstance value, long contextPointer) {
        this(Type.INSTANCE_POINTER, value.instancePointer, contextPointer);
    }

    Type getType() { return type; }

    protected Boolean getAsBoolean() { return (Boolean) value; }
    protected Long getAsInteger() { return ((Number) value).longValue(); }
    protected Double getAsFloat() { return ((Number) value).doubleValue(); }
    protected String getAsString() { return (String) value; }
    protected long getAsInstancePointer() { return (Long) value; }

    PureRustResult[] getAsArray() { return (PureRustResult[]) value; }
}
