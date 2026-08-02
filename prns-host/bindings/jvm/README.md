# Personal RNS for Kotlin, Java, and Android

> **Status: solid core, young surface.**
> This binding runs the same Rust engine as every Prns node and passes the same cross-language conformance suite on every release.
> The young part is the JVM-facing API. Its shape is a working first draft: a starting point, not the final word.
> If you are an experienced Kotlin or Java developer and something here does not feel native, that is exactly the feedback we want. Issues and PRs on API design are among the most valuable contributions right now.

The JVM SDK is a thin, typed adapter over the versioned Personal RNS C host
contract. Kotlin callers receive sealed command outcomes and cold
single-consumer `Flow` event streams. Java callers use the same classes and
`AutoCloseable` ownership with cancellable `CompletionStage` operations. Native
readiness wakes Kotlin coroutines through a conflated channel without occupying
a waiting worker thread.

```kotlin
Host(
    HostOptions(
        role = HostRole.ENDPOINT,
        identity = IdentityConfigGenerateEphemeral,
        destinations = emptyList(),
        requiredCapabilities = setOf(Capability.TCP_CLIENT),
    ),
).use { host ->
    when (val settlement = host.attachTcpClient("127.0.0.1:4242", BitrateAuto)) {
        is CommandSucceeded -> when (val outcome = settlement.outcome) {
            is CommandOutcomeInterfaceAttached -> println(outcome.`interface`)
            else -> Unit
        }
        is CommandFailed -> handleFailure(settlement.failure)
    }
}
```

```java
Host host = new Host(new HostOptions(
        HostRole.ENDPOINT,
        IdentityConfigGenerateEphemeral.INSTANCE,
        java.util.Collections.emptyList(),
        java.util.Collections.emptySet(),
        Limits.Balanced
));
host.attachTcpClientAsync("127.0.0.1:4242", BitrateAuto.INSTANCE)
    .whenComplete((settlement, failure) -> host.close());
```

Cancelling the `CompletableFuture` returned by `toCompletableFuture()` interrupts
the native wait immediately. Each application or diagnostic stream can be
claimed once, and each claimed `EventFlow` can either be collected once as a
Kotlin `Flow` or consumed through Java's `nextAsync()`. Closing a host, command,
event flow, or resource stream releases the corresponding native handle
deterministically.

Contract `safeInt` and `safeUint` fields use JVM `long`; their schema bounds
keep every value exactly representable for JavaScript interop, with `safeUint`
remaining non-negative. Exact contract `u64` fields use Kotlin `ULong`; the JNA
boundary preserves all 64 bits while the generated Kotlin surface makes
unsigned intent explicit.

Desktop applications provide `libprns_host` through the dynamic loader, the
`PRNS_HOST_LIBRARY` environment variable, or the `personal.rns.library` system
property. Release archives contain the matching library and `personal-rns`
pkg-config metadata.

Android applications use the same API and bytecode. Add JNA's Android artifact,
exclude the desktop JNA runtime selected by the Maven POM, and place the Personal
RNS libraries from the Android release artifact in the normal ABI directories:

```kotlin
implementation("io.reticulum:personal-rns:0.3.2") {
    exclude(group = "net.java.dev.jna", module = "jna")
}
implementation("net.java.dev.jna:jna:5.19.1@aar")
```

```text
src/main/jniLibs/arm64-v8a/libprns_host.so
src/main/jniLibs/armeabi-v7a/libprns_host.so
```

`CompletionStage` requires Android API 24 or core library desugaring on older
Android versions.

The Gradle wrapper is pinned to 9.6.1 with distribution checksum verification.
`./gradlew test` compiles with warnings as errors and runs the adapter against a
real native host when `-Dpersonal.rns.library=/absolute/path/libprns_host.so` is
provided.
