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

import java.util.Objects;

/**
 * A Pure date value as the literal text the Rust side printed
 * ({@code 2024-02-29}, {@code 2024-02-29T10:00:00.000+0000}, ...).
 * Distinct from {@link String} so a date can never be mistaken for text.
 * Dates currently cross the boundary from Rust to Java only.
 */
public final class PureRustDate
{
    private final String literal;

    public PureRustDate(String literal)
    {
        this.literal = Objects.requireNonNull(literal, "literal");
    }

    public String literal()
    {
        return this.literal;
    }

    @Override
    public boolean equals(Object o)
    {
        return (this == o) || ((o instanceof PureRustDate) && this.literal.equals(((PureRustDate) o).literal));
    }

    @Override
    public int hashCode()
    {
        return this.literal.hashCode();
    }

    @Override
    public String toString()
    {
        return this.literal;
    }
}
