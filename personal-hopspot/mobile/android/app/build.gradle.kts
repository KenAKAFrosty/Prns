plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
}

val releaseNoticesDirectory = layout.buildDirectory.dir("generated/release-notices/assets")
val releaseKeystore = providers.environmentVariable("PRNS_ANDROID_KEYSTORE").orNull
val releaseKeystorePassword =
    providers.environmentVariable("PRNS_ANDROID_KEYSTORE_PASSWORD").orNull
val releaseKeyAlias = providers.environmentVariable("PRNS_ANDROID_KEY_ALIAS").orNull
val releaseKeyPassword = providers.environmentVariable("PRNS_ANDROID_KEY_PASSWORD").orNull
val releaseSigningReady =
    listOf(
        releaseKeystore,
        releaseKeystorePassword,
        releaseKeyAlias,
        releaseKeyPassword,
    ).all { !it.isNullOrBlank() }
val productionSigningRequested =
    gradle.startParameter.taskNames.any {
        it.substringAfterLast(':') == "assembleProduction"
    }
val experimentalWifiDirectDisabledRequested =
    gradle.startParameter.taskNames.any {
        it.substringAfterLast(':') == "verifyExperimentalWifiDirectDisabled"
    }
val experimentalWifiDirect =
    providers.gradleProperty("prnsExperimentalWifiDirect").orNull?.also { value ->
        require(value == "true" || value == "false") {
            "prnsExperimentalWifiDirect must be true or false"
        }
    } ?: "false"
if (productionSigningRequested || experimentalWifiDirectDisabledRequested) {
    require(experimentalWifiDirect == "false") {
        "production validation cannot enable experimental Wi-Fi Direct"
    }
}
if (productionSigningRequested) {
    require(releaseSigningReady) {
        "PRNS_ANDROID_KEYSTORE, PRNS_ANDROID_KEYSTORE_PASSWORD, PRNS_ANDROID_KEY_ALIAS, and PRNS_ANDROID_KEY_PASSWORD are required"
    }
    require(file(requireNotNull(releaseKeystore)).isFile) {
        "PRNS_ANDROID_KEYSTORE does not identify a file"
    }
}
val syncReleaseNotices by tasks.registering(Copy::class) {
    val notices = rootProject.layout.projectDirectory.file("../../../THIRD_PARTY_NOTICES.md")
    from(notices)
    into(releaseNoticesDirectory)
    inputs.file(notices)
}

android {
    namespace = "org.personal.hopspot"
    compileSdk = 34
    buildFeatures {
        buildConfig = true
    }

    defaultConfig {
        applicationId = "org.personal.hopspot"
        minSdk = 19
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "org.personal.hopspot.PrnsRuntimeProbe"
        buildConfigField("boolean", "EXPERIMENTAL_WIFI_DIRECT", experimentalWifiDirect)
        ndk {
            abiFilters += listOf("armeabi-v7a", "arm64-v8a")
        }
    }

    signingConfigs {
        if (releaseSigningReady) {
            create("production") {
                storeFile = file(requireNotNull(releaseKeystore))
                storePassword = releaseKeystorePassword
                keyAlias = releaseKeyAlias
                keyPassword = releaseKeyPassword
            }
        }
    }

    buildTypes {
        create("wifiDirectLab") {
            initWith(getByName("debug"))
            applicationIdSuffix = ".wifidirectlab"
            versionNameSuffix = "-wifi-direct-lab"
            buildConfigField("boolean", "EXPERIMENTAL_WIFI_DIRECT", "true")
            matchingFallbacks += listOf("debug")
        }
        release {
            isMinifyEnabled = false
            if (releaseSigningReady) {
                signingConfig = signingConfigs.getByName("production")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }

    kotlinOptions {
        jvmTarget = "1.8"
    }

    sourceSets.getByName("main").assets.srcDir(releaseNoticesDirectory)

}

tasks.named("preBuild").configure {
    dependsOn(syncReleaseNotices)
}

tasks.register("assembleProduction") {
    notCompatibleWithConfigurationCache("signing credentials must not be serialized")
    dependsOn("assembleRelease")
}

tasks.register("verifyExperimentalWifiDirectDisabled")

dependencies {
    implementation(libs.usb.serial)
}

afterEvaluate {
    val releaseRuntimeCoordinates = configurations.getByName("releaseRuntimeClasspath")
        .incoming.resolutionResult.allComponents
        .mapNotNull { component ->
            component.moduleVersion?.takeIf { id -> id.version != "unspecified" }?.toString()
        }
        .distinct()
        .sorted()
    val baseline = rootProject.layout.projectDirectory.file("dependencies/release-runtime.tsv")
    tasks.register("verifyReleaseRuntimeDependencies") {
        inputs.file(baseline)
        inputs.property("releaseRuntimeCoordinates", releaseRuntimeCoordinates)
        doLast {
            val expected = baseline.asFile.readLines()
                .map(String::trim)
                .filter { it.isNotEmpty() && !it.startsWith("#") }
                .map { it.substringBefore('\t') }
                .sorted()
            val actual = releaseRuntimeCoordinates
            check(actual == expected) {
                "releaseRuntimeClasspath drifted.\nExpected:\n${expected.joinToString("\n")}\n" +
                    "Actual:\n${actual.joinToString("\n")}"
            }
        }
    }
}
