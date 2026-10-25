import org.gradle.api.tasks.Exec

val abis = (findProperty("copycat.abis") as String? ?: "arm64-v8a").split(",")

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.roborazzi)
}

android {
    namespace = "app.copycat"
    compileSdk = 37

    defaultConfig {
        applicationId = "app.copycat"
        minSdk = 31
        targetSdk = 36
        versionCode = 4
        versionName = "0.2.2"
        ndk { abiFilters += abis }
    }

    val releaseStore = (findProperty("copycat.signing.storeFile") as String?)?.let(::File)?.takeIf { it.isFile }
    if (releaseStore != null) {
        signingConfigs.create("release") {
            val password = File(findProperty("copycat.signing.passwordFile") as String).readText().trim()
            storeFile = releaseStore
            storePassword = password
            keyAlias = findProperty("copycat.signing.alias") as String? ?: "copycat"
            keyPassword = password
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
        }
    }

    buildFeatures { compose = true }

    testOptions {
        unitTests.isIncludeAndroidResources = true
        unitTests.all {

            it.jvmArgs("--add-exports=java.base/jdk.internal.access=ALL-UNNAMED", "--add-opens=java.base/java.io=ALL-UNNAMED")
            it.maxHeapSize = "2g"
        }
    }

    packaging { jniLibs { useLegacyPackaging = true } }

    sourceSets["main"].jniLibs.directories += "build/rustJniLibs"
}

val cargoCompile by tasks.registering(Exec::class) {
    val root = rootDir.parentFile
    workingDir = root
    commandLine(listOf("cargo", "ndk") + abis.flatMap { listOf("-t", it) } + listOf("-P", "31", "-o", "${projectDir}/build/rustJniLibs", "build", "-p", "copycat-android", "--release"))
    inputs.dir(File(root, "platform/android/src"))
    inputs.dir(File(root, "core"))
    inputs.property("abis", abis)
    outputs.dir("build/rustJniLibs")
}

val abiTriples = mapOf("arm64-v8a" to "aarch64-linux-android", "x86_64" to "x86_64-linux-android")

val cargoTargetDir = System.getenv("CARGO_TARGET_DIR")?.takeIf { it.isNotBlank() }?.let(::File) ?: File(rootDir.parentFile, "target")
val cargoBuild by tasks.registering(Copy::class) {
    dependsOn(cargoCompile)
    val launchers = abis.associateWith { File(cargoTargetDir, "${abiTriples.getValue(it)}/release/copycat-launch") }
    doFirst {
        launchers.values.filterNot { it.isFile }.forEach { throw GradleException("找不到降权启动器：$it") }
    }
    for ((abi, launcher) in launchers) {
        from(launcher) {
            into(abi)
            rename { "libcopycatd.so" }
        }
    }
    into("build/rustJniLibs")
}
tasks.named("preBuild") { dependsOn(cargoBuild) }

dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
    implementation(libs.compose.animation)
    implementation(libs.compose.ui.tooling.preview)
    debugImplementation(libs.compose.ui.tooling)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.kotlinx.coroutines.android)

    implementation(libs.camera.camera2)
    implementation(libs.camera.lifecycle)
    implementation(libs.camera.compose)
    implementation(libs.zxing.core)

    testImplementation(libs.junit)
    testImplementation(libs.robolectric)
    testImplementation(libs.roborazzi)
    testImplementation(libs.roborazzi.compose)
    testImplementation(libs.compose.ui.test.junit4)
    debugImplementation(libs.compose.ui.test.manifest)
}
