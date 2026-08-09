package rs.reticulum.prns

import java.nio.file.Path

data class Limits(
    val pendingCommands: Long,
    val applicationEvents: Long,
    val retainedEventBytes: Long,
    val diagnostics: Long,
) {
    companion object {
        @JvmField
        val Balanced = Limits(
            pendingCommands = HostContract.BALANCED_PENDING_COMMANDS.toLong(),
            applicationEvents = HostContract.BALANCED_APPLICATION_EVENTS.toLong(),
            retainedEventBytes = HostContract.BALANCED_RETAINED_EVENT_BYTES.toLong(),
            diagnostics = HostContract.BALANCED_DIAGNOSTICS.toLong(),
        )
    }
}

data class HostOptions @JvmOverloads constructor(
    val role: HostRole,
    val identity: IdentityConfig,
    val destinations: List<DestinationConfig>,
    val requiredCapabilities: Set<Capability> = emptySet(),
    val limits: Limits = Limits.Balanced,
    val persistence: PersistenceConfig = PersistenceConfigEphemeral,
) {
    companion object {
        @JvmStatic
        fun persistentEndpoint(root: Path): HostOptions = HostOptions(
            role = HostRole.ENDPOINT,
            identity = IdentityConfigLoadOrCreate(root.resolve("identity").toString()),
            destinations = emptyList(),
            persistence = PersistenceConfigDirectory(root.resolve("state").toString()),
        )
    }
}
