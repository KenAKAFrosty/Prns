import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    kotlin("jvm") version "2.4.10"
    `java-library`
    `maven-publish`
    signing
}

group = "rs.reticulum"
version = "0.3.7"

repositories {
    mavenCentral()
}

dependencies {
    api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.11.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-jdk8:1.11.0")
    implementation("net.java.dev.jna:jna:5.19.1")
    testImplementation(kotlin("test"))
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.11.0")
}

kotlin {
    jvmToolchain(17)
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
        allWarningsAsErrors = true
        javaParameters = true
    }
}

java {
    sourceCompatibility = JavaVersion.VERSION_1_8
    targetCompatibility = JavaVersion.VERSION_1_8
    withJavadocJar()
    withSourcesJar()
}

tasks.test {
    useJUnitPlatform()
}

tasks.processResources {
    from(rootProject.file("../../distribution/PACKAGE.md"))
    from(rootProject.file("../../../LICENSE-APACHE"))
    from(rootProject.file("../../../LICENSE-MIT"))
}

publishing {
    publications {
        create<MavenPublication>("maven") {
            from(components["java"])
            pom {
                name = "Personal RNS"
                description = "Kotlin and Java host SDK for Personal RNS"
                url = "https://github.com/KenAKAFrosty/Prns"
                licenses {
                    license {
                        name = "MIT"
                        url = "https://opensource.org/license/mit"
                    }
                    license {
                        name = "Apache-2.0"
                        url = "https://www.apache.org/licenses/LICENSE-2.0"
                    }
                }
                developers {
                    developer {
                        id = "personal-rns-contributors"
                        name = "Personal RNS contributors"
                        url = "https://github.com/KenAKAFrosty/Prns/graphs/contributors"
                    }
                }
                scm {
                    connection = "scm:git:https://github.com/KenAKAFrosty/Prns.git"
                    developerConnection = "scm:git:ssh://git@github.com/KenAKAFrosty/Prns.git"
                    url = "https://github.com/KenAKAFrosty/Prns"
                }
            }
        }
    }
    repositories {
        maven {
            name = "staging"
            url = uri(layout.buildDirectory.dir("staging-repository"))
        }
    }
}

signing {
    val key = providers.environmentVariable("MAVEN_SIGNING_KEY")
    val password = providers.environmentVariable("MAVEN_SIGNING_PASSWORD")
    if (key.isPresent) {
        useInMemoryPgpKeys(key.get(), password.orNull)
        sign(publishing.publications)
    }
}
