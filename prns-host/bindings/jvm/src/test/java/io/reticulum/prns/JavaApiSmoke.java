package io.reticulum.prns;

import java.util.Collections;

/**
 * Compile-time proof that the published contract is an ordinary Java API.
 *
 * Kotlin unsigned types would make these constructors and getters inaccessible
 * through JVM name mangling, so this deliberately lives in Java.
 */
public final class JavaApiSmoke {
    private JavaApiSmoke() {}

    public static void compileContractSurface() {
        Limits limits = new Limits(64L, 256L, 8L * 1024L * 1024L, 1024L);
        HostOptions options = new HostOptions(
                HostRole.ENDPOINT,
                IdentityConfigGenerateEphemeral.INSTANCE,
                Collections.emptyList(),
                Collections.emptySet(),
                limits
        );
        Bitrate bitrate = new BitrateBitsPerSecond(1_000_000L);

        if (options.getLimits().getPendingCommands() != 64L) {
            throw new AssertionError("u64 getter is not Java-accessible");
        }
        if (((BitrateBitsPerSecond) bitrate).getValue() != 1_000_000L) {
            throw new AssertionError("generated union is not Java-accessible");
        }

        // Method references prove Java can settle commands and consume events
        // without manually constructing Kotlin continuations.
        java.util.function.Function<Command, CommandSettlement> await =
                Command::awaitBlocking;
        java.util.function.Function<EventFlow<ApplicationEvent>, ApplicationEvent> next =
                EventFlow::nextBlocking;
        if (await == null || next == null) {
            throw new AssertionError("unreachable");
        }
    }
}
