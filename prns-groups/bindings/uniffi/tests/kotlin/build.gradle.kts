import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    kotlin("jvm") version "2.4.10"
    application
}
repositories { mavenCentral() }
dependencies {
    implementation("net.java.dev.jna:jna:5.18.1")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")
}
java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}
kotlin {
    compilerOptions.jvmTarget.set(JvmTarget.JVM_17)
    sourceSets.main { kotlin.srcDirs("generated", "consumer") }
}
application { mainClass.set("ConsumerKt") }
tasks.named<JavaExec>("run") {
    systemProperty("jna.library.path", providers.gradleProperty("nativeLibraryDirectory").get())
}
