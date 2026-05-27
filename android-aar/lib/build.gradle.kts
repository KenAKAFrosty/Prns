import java.io.File
import java.util.Properties

plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

// Android `.aar` packaging for `personal-rns-ffi`. Bundles the committed
// uniffi-generated Kotlin tree plus the per-Android-ABI `personal-rns-ffi`
// cdylib (built by `cargo ndk`) into one self-contained library archive a
// downstream Gradle consumer can `implementation files('…/lib-release.aar')`.
//
// Local-only by design: no Maven publish, no signing. This module exists to
// validate the package shape end to end; remote publishing is a later step.

android {
    namespace = "io.personal.rns.aar"
    compileSdk = 35

    defaultConfig {
        minSdk = 24
        consumerProguardFiles("consumer-rules.pro")
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

    sourceSets {
        getByName("main") {
            // Compile the committed uniffi-generated Kotlin directly out of the
            // bindings crate's `generated/` tree, so a UDL-refresh cycle
            // propagates into this `.aar` with no separate copy step.
            kotlin.srcDir(rootDir.resolve("../personal-rns-ffi/generated/kotlin"))
            jniLibs.srcDir(layout.buildDirectory.dir("nativeLibs"))
        }
    }
}

dependencies {
    // JNA's `@aar` variant ships the per-ABI native-dispatch libraries the
    // uniffi-generated Kotlin's `Native.load("personal_rns_ffi", …)` needs on
    // Android. uniffi 0.28's Kotlin is pinned to the JNA 5.x ABI.
    implementation("net.java.dev.jna:jna:5.14.0@aar")
}

// Build the per-ABI cdylib for `personal-rns-ffi` via `cargo ndk` into
// `build/nativeLibs/<abi>/libpersonal_rns_ffi.so`; the Android Gradle plugin
// picks them up through `jniLibs.srcDir(…)` above.
//
// A hand-rolled `Exec` shell-out to `cargo-ndk` (a documented build-time
// prerequisite) is preferred over importing a third-party Rust-Gradle plugin:
// one auditable command keeps the transitive surface small.
val abiTargets = mapOf(
    "arm64-v8a" to "aarch64-linux-android",
    "armeabi-v7a" to "armv7-linux-androideabi",
    "x86" to "i686-linux-android",
    "x86_64" to "x86_64-linux-android",
)

/**
 * Resolve the Android NDK home `cargo-ndk` should use. Honors, in order:
 *   1. `ANDROID_NDK_HOME` env var,
 *   2. `ANDROID_NDK_ROOT` env var,
 *   3. `ndk.dir=…` in `local.properties`,
 *   4. highest-versioned `ndk/<version>/` under `sdk.dir=…` in `local.properties`,
 *   5. highest-versioned `ndk/<version>/` under `ANDROID_SDK_ROOT` / `ANDROID_HOME`.
 * Throws with remediation guidance rather than baking an absolute path in.
 */
fun resolveAndroidNdkHome(): String {
    System.getenv("ANDROID_NDK_HOME")?.takeIf { it.isNotBlank() }?.let { return it }
    System.getenv("ANDROID_NDK_ROOT")?.takeIf { it.isNotBlank() }?.let { return it }

    val localProperties = Properties().apply {
        val file = rootDir.resolve("local.properties")
        if (file.isFile) {
            file.inputStream().use { stream -> load(stream) }
        }
    }
    localProperties.getProperty("ndk.dir")
        ?.takeIf { it.isNotBlank() }
        ?.let { return it }

    fun highestVersionedNdk(sdkRoot: File): String? {
        val ndkParent = sdkRoot.resolve("ndk")
        if (!ndkParent.isDirectory) return null
        return ndkParent.listFiles { entry -> entry.isDirectory }
            ?.maxByOrNull { it.name }
            ?.absolutePath
    }

    localProperties.getProperty("sdk.dir")
        ?.takeIf { it.isNotBlank() }
        ?.let { sdk -> highestVersionedNdk(File(sdk)) }
        ?.let { return it }

    listOf("ANDROID_SDK_ROOT", "ANDROID_HOME").forEach { envName ->
        System.getenv(envName)
            ?.takeIf { it.isNotBlank() }
            ?.let { sdk -> highestVersionedNdk(File(sdk)) }
            ?.let { return it }
    }

    throw GradleException(
        """
        |personal-rns-ffi-android-aar: cannot locate an Android NDK.
        |Set one of: ANDROID_NDK_HOME, ANDROID_NDK_ROOT, ndk.dir/sdk.dir in
        |local.properties, or ANDROID_SDK_ROOT / ANDROID_HOME (with an
        |ndk/<version>/ under it) before `./gradlew :lib:assembleRelease`.
        """.trimMargin(),
    )
}

val cargoNdkBuild by tasks.registering(Exec::class) {
    group = "build"
    description = "Build personal-rns-ffi cdylib for every Android ABI via cargo-ndk."

    val workspaceRoot = rootDir.resolve("..").canonicalFile
    workingDir = workspaceRoot

    environment("ANDROID_NDK_HOME", resolveAndroidNdkHome())

    val outDir = layout.buildDirectory.dir("nativeLibs").get().asFile
    doFirst { outDir.mkdirs() }

    val cmd = mutableListOf("cargo", "ndk", "--platform", "24")
    abiTargets.keys.forEach { abi -> cmd += listOf("-t", abi) }
    cmd += listOf("-o", outDir.absolutePath, "build", "--release", "-p", "personal-rns-ffi")
    commandLine = cmd

    inputs.dir(workspaceRoot.resolve("personal-rns-ffi/src"))
    inputs.file(workspaceRoot.resolve("personal-rns-ffi/Cargo.toml"))
    inputs.file(workspaceRoot.resolve("personal-rns-ffi/build.rs"))
    inputs.dir(workspaceRoot.resolve("personal-rns/src"))
    outputs.dir(outDir)
}

tasks.named("preBuild") { dependsOn(cargoNdkBuild) }
