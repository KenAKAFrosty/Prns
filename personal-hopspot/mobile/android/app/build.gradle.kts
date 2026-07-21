plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
}

val releaseNoticesDirectory = layout.buildDirectory.dir("generated/release-notices/assets")
val syncReleaseNotices by tasks.registering(Copy::class) {
    val notices = rootProject.layout.projectDirectory.file("../../../THIRD_PARTY_NOTICES.md")
    from(notices)
    into(releaseNoticesDirectory)
    inputs.file(notices)
}

android {
    namespace = "org.personal.hopspot"
    compileSdk = 34

    defaultConfig {
        applicationId = "org.personal.hopspot"
        minSdk = 19
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "org.personal.hopspot.PrnsRuntimeProbe"
        ndk {
            abiFilters += listOf("armeabi-v7a", "arm64-v8a")
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

    sourceSets.getByName("main").assets.srcDir(releaseNoticesDirectory)

}

tasks.named("preBuild").configure {
    dependsOn(syncReleaseNotices)
}

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
