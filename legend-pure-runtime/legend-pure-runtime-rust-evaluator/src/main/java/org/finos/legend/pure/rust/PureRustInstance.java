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

public class PureRustInstance
{
    protected final long instancePointer;
    private final PureRustEvaluator owner;

    protected PureRustInstance(long instancePointer, PureRustEvaluator owner)
    {
        this.instancePointer = instancePointer;
        this.owner = owner;
    }

    public String getClassifier()
    {
        return this.owner.getClassifier(this);
    }

    public <T> T getProperty(String propertyName, Object... args)
    {
        return this.owner.evaluateProperty(this.instancePointer, propertyName, args);
    }
}
