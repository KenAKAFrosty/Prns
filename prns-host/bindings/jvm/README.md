# Personal RNS for Kotlin, Java, and Android

The JVM SDK is a thin, typed adapter over the versioned Personal RNS C host
contract. Kotlin callers receive sealed command outcomes and cold
single-consumer `Flow` event streams. Java callers use the same classes and
`AutoCloseable` ownership with direct blocking bridges.

```kotlin
Host(
    HostOptions(
        role = HostRole.ENDPOINT,
        identity = IdentityConfigGenerateEphemeral,
        destinations = emptyList(),
        requiredCapabilities = setOf(Capability.TCP_CLIENT),
    ),
).use { host ->
    val command = host.execute(
        HostCommandAttachTcpClient("127.0.0.1:4242", BitrateAuto),
    )
    command.use {
        when (val settlement = command.await()) {
            is CommandSucceeded -> when (val outcome = settlement.outcome) {
                is CommandOutcomeInterfaceAttached -> println(outcome.`interface`)
                else -> Unit
            }
            is CommandFailed -> handleFailure(settlement.failure)
        }
    }
}
```

```java
try (Host host = new Host(new HostOptions(
        HostRole.ENDPOINT,
        IdentityConfigGenerateEphemeral.INSTANCE,
        java.util.Collections.emptyList(),
        java.util.Collections.emptySet(),
        Limits.Balanced
))) {
    try (Command command = host.execute(
            new HostCommandAttachTcpClient("127.0.0.1:4242", BitrateAuto.INSTANCE)
    )) {
        CommandSettlement settlement = command.awaitBlocking();
    }
}
```

Cancellation interrupts the native wait immediately. Each application or
diagnostic stream can be claimed once, and each claimed `EventFlow` can be
either collected once as a Kotlin `Flow` or read through Java's
`nextBlocking()`. Closing a host, command, event flow, or resource stream
releases the corresponding native handle deterministically.

Contract `u64` fields use JVM `long` so constructors and getters remain ordinary
Java methods. The full 64-bit ABI representation is preserved; Java callers
that operate near the high bit can use `Long.compareUnsigned` and
`Long.toUnsignedString`.

Desktop applications provide `libprns_host` through the dynamic loader, the
`PRNS_HOST_LIBRARY` environment variable, or the `personal.rns.library` system
property. Release archives contain the matching library and `personal-rns`
pkg-config metadata.

Android applications use the same API and bytecode. Add JNA's Android artifact,
exclude the desktop JNA runtime selected by the Maven POM, and place the Personal
RNS libraries from the Android release artifact in the normal ABI directories:

```kotlin
implementation("io.reticulum:personal-rns:0.3.0") {
    exclude(group = "net.java.dev.jna", module = "jna")
}
implementation("net.java.dev.jna:jna:5.19.1@aar")
```

```text
src/main/jniLibs/arm64-v8a/libprns_host.so
src/main/jniLibs/armeabi-v7a/libprns_host.so
```

The Gradle wrapper is pinned to 9.6.1 with distribution checksum verification.
`./gradlew test` compiles with warnings as errors and runs the adapter against a
real native host when `-Dpersonal.rns.library=/absolute/path/libprns_host.so` is
provided.
