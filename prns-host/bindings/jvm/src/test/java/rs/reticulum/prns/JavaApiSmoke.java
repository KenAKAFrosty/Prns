package rs.reticulum.prns;

import java.util.Collections;
import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.CompletionStage;
import java.util.function.BiFunction;
import java.util.function.Function;

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

        BiFunction<Host, HostCommand, CompletionStage<CommandSettlement>> execute =
                Host::executeAsync;
        Function<EventFlow<ApplicationEvent>, CompletionStage<ApplicationEvent>> next =
                EventFlow::nextAsync;
        BiFunction<ResourceUpload, Bytes, CompletionStage<Void>> write =
                ResourceUpload::writeAsync;
        Function<ResourceUpload, CompletionStage<CommandSettlement>> finish =
                ResourceUpload::finishAsync;
        if (execute == null || next == null || write == null || finish == null) {
            throw new AssertionError("unreachable");
        }

        Set<String> methods = new HashSet<>();
        for (java.lang.reflect.Method method : Host.class.getMethods()) {
            methods.add(method.getName());
        }
        Set<String> required = new HashSet<>();
        Collections.addAll(
                required,
                "allowRequesterAsync",
                "announceAsync",
                "attachInterfaceAsync",
                "attachTcpClientAsync",
                "attachTcpServerAsync",
                "attachUdpAsync",
                "closeLinkAsync",
                "detachInterfaceAsync",
                "establishLinkAsync",
                "identifyAsync",
                "requestAsync",
                "requestPathAsync",
                "respondAsync",
                "sendChannelMessageAsync",
                "sendLinkPacketAsync",
                "sendResourceAsync",
                "sendSinglePacketAsync",
                "setDestinationResourceStrategyAsync",
                "setLinkResourceStrategyAsync"
        );
        if (!methods.containsAll(required)) {
            required.removeAll(methods);
            throw new AssertionError("missing Java async helpers: " + required);
        }
    }
}
