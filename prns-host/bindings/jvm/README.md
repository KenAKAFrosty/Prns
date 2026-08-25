# Personal RNS for Kotlin, Java, and Android

> **SDK preview: implemented, tested in-tree, and awaiting polished distribution.**
> This adapter runs the same Rust engine as every Prns node and is exercised by the repository's registered live JVM conformance suite.
> Maven packaging, desktop and Android native assets, and public-package qualification are active release work, so a source checkout is currently the supported evaluation path.
> If you are experienced with Kotlin, Java, Android, or Maven design, help making this feel completely at home in those ecosystems would be especially valuable.

## Evaluate the current source

On Linux, the registered suite builds the matching native capsule and runs the complete persistent two-node journey through the Gradle project:

```console
python3 validation/run.py run --suite host-jvm-contract
```

The intended public delivery is an `rs.reticulum:personal-rns` Maven package paired with desktop and Android native assets. Until those packages have completed public qualification, do not assume Maven Central contains the adapter from this checkout. See the [SDK guide](../../../docs/sdks.md#native-sdk-previews) for the shared release posture and contribution path.

## API shape

The JVM SDK is a thin, typed adapter over the versioned Personal RNS C host contract. Kotlin callers receive sealed command outcomes and cold single-consumer `Flow` event streams. Java callers use the same classes and `AutoCloseable` ownership with cancellable `CompletionStage` operations. Native readiness wakes Kotlin coroutines through a conflated channel without occupying a waiting worker thread.

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

Cancelling the `CompletableFuture` returned by `toCompletableFuture()` interrupts the native wait immediately. Each application or diagnostic stream can be claimed once, and each claimed `EventFlow` can either be collected once as a Kotlin `Flow` or consumed through Java's `nextAsync()`. Closing a host, command, event flow, or resource stream releases the corresponding native handle deterministically.

Contract `safeInt` and `safeUint` fields use JVM `long`; their schema bounds keep every value exactly representable for JavaScript interop, with `safeUint` remaining non-negative. Exact contract `u64` fields use Kotlin `ULong`; the JNA boundary preserves all 64 bits while the generated Kotlin surface makes unsigned intent explicit.

Source-built desktop applications provide `libprns_host` through the dynamic loader, the `PRNS_HOST_LIBRARY` environment variable, or the `personal.rns.library` system property. The planned release archives contain the matching library and `personal-rns` pkg-config metadata.

Android applications use the same API and bytecode. After public package promotion, the intended Gradle shape adds JNA's Android artifact, excludes the desktop JNA runtime selected by the Maven POM, and places the Personal RNS libraries from the Android release artifact in the normal ABI directories:

```kotlin
implementation("rs.reticulum:personal-rns:0.3.7") {
    exclude(group = "net.java.dev.jna", module = "jna")
}
implementation("net.java.dev.jna:jna:5.19.1@aar")
```

```text
src/main/jniLibs/arm64-v8a/libprns_host.so
src/main/jniLibs/armeabi-v7a/libprns_host.so
```

Applications that must create the transport themselves can attach it as an
ordinary Pipe. The controller keeps transport creation in the application's
coroutine; native code publishes requests and never calls into the JVM to open
a descriptor:

```kotlin
host.beginSuppliedPipe(
    name = "protected-uplink",
    respawnDelayMillis = 5_000,
    bitrate = BitrateAuto,
).use { pipe ->
    when (val attached = pipe.awaitAttachment()) {
        is CommandSucceeded -> pipe.serve {
            connectProtectedParcelFileDescriptor().detachFd()
        }
        is CommandFailed -> handleFailure(attached.failure)
    }
}
```

The engine owns and closes every non-negative descriptor accepted from
`serve`. Return `SUPPLIED_PIPE_DECLINED` when no connection is available; a new
request arrives after the respawn delay. Cancelling `serve` releases its current
request, while closing the controller also detaches the interface. This
operation is reported by the
`SUPPLIED_PIPE` capability and is POSIX-only, including Android and Apple/Linux
JVM targets.

`CompletionStage` requires Android API 24 or core library desugaring on older Android versions.

The Gradle wrapper is pinned to 9.6.1 with distribution checksum verification. `./gradlew test` compiles with warnings as errors and runs the adapter against a real native host when `-Dpersonal.rns.library=/absolute/path/libprns_host.so` is provided.
