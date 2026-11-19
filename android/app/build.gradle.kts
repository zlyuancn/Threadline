plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "cn.zly.threadline"
    compileSdk = 35

    defaultConfig {
        applicationId = "cn.zly.threadline"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.0.1-spike"

        ndk {
            // 首发只发 arm64-v8a（设计「风险」节：四个 ABI 的 .so 会让 APK 太大）。
            abiFilters += listOf("arm64-v8a")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    sourceSets["main"].jniLibs.srcDirs(
        // Rust 产物落在仓库 tmp/（gitignore），不进 android/ 源码树。
        file("../../tmp/data/jniLibs"),
    )
}

dependencies {
    // UniFFI 的 Kotlin 绑定底层走 JNA：必须是 aar 变体，否则缺 libjnidispatch.so 直接 UnsatisfiedLinkError。
    implementation(libs.jna) { artifact { type = "aar" } }
}
