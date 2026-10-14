plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "dev.cortex.remote"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.cortex.remote"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"

        // The core is built for these two only (scripts/mobile-android.sh);
        // JNA's own library for older ones would be dead weight.
        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    buildTypes {
        release {
            // Shrinks the Kotlin and the resources. The core is reached
            // through JNA by name, so its bindings are kept whole.
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    packaging {
        resources.excludes += "/META-INF/{AL2.0,LGPL2.1}"
    }
}

composeCompiler {
    // The core's records are plain values the screens never change: the
    // compiler is told so, and skips a row whose item is the same.
    stabilityConfigurationFiles.add(project.layout.projectDirectory.file("compose-stability.conf"))

    // `./gradlew :app:assembleRelease -Pcompose.reports` writes what the
    // compiler found skippable to build/compose.
    if (project.hasProperty("compose.reports")) {
        reportsDestination.set(project.layout.buildDirectory.dir("compose"))
        metricsDestination.set(project.layout.buildDirectory.dir("compose"))
    }
}

kotlin {
    jvmToolchain(21)

    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2025.09.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.activity:activity-compose:1.11.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.9.4")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.4")
    implementation("androidx.core:core-ktx:1.17.0")

    // Shows the ChatGPT sign-in page over the app, which keeps running.
    implementation("androidx.browser:browser:1.9.0")

    // Runs the networks' rest while the phone is idle.
    implementation("androidx.work:work-runtime-ktx:2.10.5")

    // The Rust core is called through JNA.
    implementation("net.java.dev.jna:jna:5.17.0@aar")

    // Scans the pairing code, with a screen of its own and no camera
    // permission to ask for.
    implementation("com.google.android.gms:play-services-code-scanner:16.1.0")

    testImplementation("junit:junit:4.13.2")
}
