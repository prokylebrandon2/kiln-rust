plugins {
    id("com.android.application")
}

android {
    namespace = "dev.kiln.app"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.kiln.app"
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        // Only the ABIs the CI workflow cross-compiles the `kiln` binary for.
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }

    // The Kiln CLI is shipped as lib/<abi>/libkiln.so and executed from the app's native library
    // directory. That directory only contains real files when libraries are extracted at install.
    packaging { jniLibs { useLegacyPackaging = true } }

    val keystore: String? = System.getenv("KILN_KEYSTORE_FILE")
    signingConfigs {
        if (keystore != null) {
            create("release") {
                storeFile = file(keystore)
                storePassword = System.getenv("KILN_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("KILN_KEY_ALIAS")
                keyPassword = System.getenv("KILN_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // Without a keystore the release APK is built unsigned (and cannot be installed).
            if (keystore != null) signingConfig = signingConfigs.getByName("release")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
