package io.reticulum.prns

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

data class HostOptions(
    val role: HostRole,
    val identity: IdentityConfig,
    val destinations: List<DestinationConfig>,
    val requiredCapabilities: Set<Capability> = emptySet(),
    val limits: Limits = Limits.Balanced,
)
