plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "io.orivo.saf"
    compileSdk = 36

    defaultConfig {
        minSdk = 24
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
}

dependencies {
    // All three are already in every Tauri Android build: Jackson is how `Invoke`
    // reads and writes its payload, `androidx.activity` is where `ActivityResult`
    // comes from — the Tauri library holds it as an `implementation` dependency,
    // so it is in the APK but not on this project's compile classpath — and the
    // Tauri library itself is where `Plugin` and the picker plumbing live.
    implementation("com.fasterxml.jackson.core:jackson-databind:2.15.3")
    implementation("androidx.activity:activity:1.5.1")
    implementation(project(":tauri-android"))
}
