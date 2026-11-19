// 版本一律钉死在版本目录里（`gradle/libs.versions.toml`），落地后不再随上游漂移。
plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.kotlin.android) apply false
}
