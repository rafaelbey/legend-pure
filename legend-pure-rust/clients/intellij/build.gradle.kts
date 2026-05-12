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

import org.jetbrains.intellij.platform.gradle.TestFrameworkType

plugins {
    id("java")
    // IDEA 2025.2.1 is compiled with Kotlin 2.2.0 metadata; the compiler
    // we use must be ≥ 2.2.0 to read its bytecode.
    id("org.jetbrains.kotlin.jvm") version "2.2.0"
    // 2.1.0 had a parser bug against the 2025.x product-info.json shape
    // (`Index: 1, Size: 1` in resolveIdeHomeVariable). 2.6.0+ is required.
    id("org.jetbrains.intellij.platform") version "2.6.0"
}

group = providers.gradleProperty("pluginGroup").get()
version = providers.gradleProperty("pluginVersion").get()

repositories {
    mavenCentral()
    intellijPlatform { defaultRepositories() }
}

kotlin {
    jvmToolchain(providers.gradleProperty("jdkVersion").get().toInt())
}

dependencies {
    intellijPlatform {
        create(
            providers.gradleProperty("platformType").get(),
            providers.gradleProperty("platformVersion").get(),
        )
        // Bundled IntelliJ plugins this plugin depends on.
        bundledPlugins(
            providers.gradleProperty("platformBundledPlugins")
                .map { it.split(",").map(String::trim).filter(String::isNotEmpty) },
        )
        plugins(
            providers.gradleProperty("platformPlugins")
                .map { it.split(",").map(String::trim).filter(String::isNotEmpty) },
        )

        // Bundled IDE modules that aren't auto-resolved by
        // `create(...)`. The DAP module ships inside the unified
        // IDE distribution (`lib/modules/intellij.platform.dap.jar`)
        // but its classes (`com.intellij.platform.dap.*`) aren't on
        // the default plugin classpath — `bundledModule` adds them
        // explicitly. `@ApiStatus.Experimental` in 2025.3; expect
        // signature drift on upgrades.
        bundledModule("intellij.platform.dap")

        // instrumentationTools() removed: bundled by default in IntelliJ
        // Platform Gradle Plugin 2.6+.
        pluginVerifier()
        zipSigner()
        testFramework(TestFrameworkType.Platform)
    }

    testImplementation("junit:junit:4.13.2")
}

intellijPlatform {
    pluginConfiguration {
        version = providers.gradleProperty("pluginVersion")

        ideaVersion {
            sinceBuild = providers.gradleProperty("pluginSinceBuild")
            untilBuild = providers.gradleProperty("pluginUntilBuild")
        }
    }
}

tasks {
    wrapper {
        gradleVersion = "8.10.2"
    }
}
