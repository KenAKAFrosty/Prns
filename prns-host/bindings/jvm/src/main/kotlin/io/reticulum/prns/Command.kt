package io.reticulum.prns

import com.sun.jna.Pointer
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock as withSuspendLock
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

sealed interface CommandSettlement

data class CommandSucceeded(val outcome: CommandOutcome) : CommandSettlement

data class CommandFailed(
    val failure: CommandFailure,
) : CommandSettlement

class Command internal constructor(pointer: Pointer) : AutoCloseable {
    private val stateLock = ReentrantLock()
    private val waitLock = ReentrantLock()
    private val asyncWait = Mutex()
    private val readiness: NativeReadiness
    private var pointer: Pointer? = pointer

    init {
        try {
            readiness = NativeReadiness.command(pointer)
        } catch (failure: Throwable) {
            NativeApi.library.prns_command_release(pointer)
            throw failure
        }
    }

    suspend fun await(): CommandSettlement = asyncWait.withSuspendLock {
        try {
            awaitSettlement()
        } catch (failure: CancellationException) {
            stateLock.withLock {
                pointer?.let(NativeApi.library::prns_command_interrupt_wait)
            }
            throw failure
        }
    }

    private suspend fun awaitSettlement(): CommandSettlement {
        while (true) {
            currentCoroutineContext().ensureActive()
            val settlement = poll()
            if (settlement != null) {
                return settlement
            }
            readiness.await()
        }
    }

    private fun poll(): CommandSettlement? = waitLock.withLock {
        val nativePointer = stateLock.withLock {
            pointer
                ?: throw StatusException("command", Status.STOPPED)
        }
        val result = NativeCommandResult()
        result.structSize = SizeT(result.size().toLong())
        result.write()
        val status = Status.fromRawValue(
            NativeApi.library.prns_command_wait(
                nativePointer,
                0,
                result,
            ),
        ) ?: Status.BACKEND_FAILED
        if (status == Status.TIMED_OUT) {
            return@withLock null
        }
        if (status != Status.OK) {
            throw StatusException("waitCommand", status)
        }
        result.read()
        decodeCommandSettlement(result)
    }

    override fun close() {
        val nativePointer = stateLock.withLock {
            val current = pointer
            pointer = null
            current?.let(NativeApi.library::prns_command_interrupt_wait)
            current
        }
        if (nativePointer != null) {
            waitLock.withLock {
                readiness.close()
                NativeApi.library.prns_command_release(nativePointer)
            }
        }
    }
}

private fun decodeCommandSettlement(result: NativeCommandResult): CommandSettlement {
    if (result.failure != 0) {
        val kind = CommandFailureKind.fromRawValue(result.failure)
            ?: throw StatusException("decodeCommandFailure", Status.BACKEND_FAILED)
        return CommandFailed(
            failure = decodeCommandFailure(kind, copyString(result.detail)),
        )
    }
    val value = copyBytes(result.value)
    val outcome = when (
        CommandOutcomeKind.fromRawValue(result.outcome)
            ?: throw StatusException("decodeCommandOutcome", Status.BACKEND_FAILED)
    ) {
        CommandOutcomeKind.ANNOUNCED -> CommandOutcomeAnnounced
        CommandOutcomeKind.PACKET_DELIVERED -> {
            val evidence = DeliveryEvidenceKind.fromRawValue(result.evidence)
                ?: throw StatusException("decodeDeliveryEvidence", Status.BACKEND_FAILED)
            val packetHash = when (evidence) {
                DeliveryEvidenceKind.RESPONSE -> {
                    if (value.isNotEmpty()) {
                        throw StatusException(
                            "decodeResponseEvidence",
                            Status.BACKEND_FAILED,
                        )
                    }
                    null
                }
                DeliveryEvidenceKind.EXPLICIT_PROOF,
                DeliveryEvidenceKind.IMPLICIT_PROOF,
                -> PacketHash(value)
            }
            CommandOutcomePacketDelivered(
                rttMillis = result.rttMillis,
                evidence = evidence,
                packetHash = packetHash,
            )
        }
        CommandOutcomeKind.LINK_CLOSE_QUEUED -> CommandOutcomeLinkCloseQueued
        CommandOutcomeKind.INTERFACE_ATTACHED -> CommandOutcomeInterfaceAttached(
            InterfaceId(value),
        )
        CommandOutcomeKind.INTERFACE_DETACHED -> CommandOutcomeInterfaceDetached(
            InterfaceId(value),
        )
        CommandOutcomeKind.LINK_ESTABLISHED -> CommandOutcomeLinkEstablished(
            LinkId(value),
            result.rttMillis,
        )
        CommandOutcomeKind.PATH_DISCOVERED -> {
            if (value.size != 1) {
                throw StatusException("decodePathHops", Status.BACKEND_FAILED)
            }
            CommandOutcomePathDiscovered(value[0].toUByte().toInt())
        }
        CommandOutcomeKind.IDENTIFIED -> CommandOutcomeIdentified
        CommandOutcomeKind.RESPONSE_RECEIVED -> CommandOutcomeResponseReceived(
            Bytes(value),
            result.rttMillis,
        )
        CommandOutcomeKind.RESPONSE_SENT -> CommandOutcomeResponseSent(
            result.rttMillis,
        )
        CommandOutcomeKind.RESOURCE_SENT -> CommandOutcomeResourceSent
        CommandOutcomeKind.RESOURCE_STRATEGY_SET -> CommandOutcomeResourceStrategySet
        CommandOutcomeKind.REQUESTER_ALLOWED -> CommandOutcomeRequesterAllowed
    }
    return CommandSucceeded(outcome)
}

private fun decodeCommandFailure(
    kind: CommandFailureKind,
    detail: String,
): CommandFailure = when (kind) {
    CommandFailureKind.NODE_STOPPED -> CommandFailureNodeStopped
    CommandFailureKind.BUSY -> CommandFailureBusy
    CommandFailureKind.PAYLOAD_TOO_LARGE -> CommandFailurePayloadTooLarge
    CommandFailureKind.UNKNOWN_DESTINATION -> CommandFailureUnknownDestination
    CommandFailureKind.NOT_SINGLE_DESTINATION -> CommandFailureNotSingleDestination
    CommandFailureKind.ANNOUNCE_APP_DATA_TOO_LONG -> CommandFailureAnnounceAppDataTooLong
    CommandFailureKind.UNKNOWN_INTERFACE -> CommandFailureUnknownInterface
    CommandFailureKind.NO_ROUTE_TO_DESTINATION -> CommandFailureNoRouteToDestination
    CommandFailureKind.NOT_DIRECTLY_REACHABLE -> CommandFailureNotDirectlyReachable
    CommandFailureKind.PACKET_CULLED -> CommandFailurePacketCulled
    CommandFailureKind.DELIVERY_TIMED_OUT -> CommandFailureDeliveryTimedOut
    CommandFailureKind.INVALID_BITRATE -> CommandFailureInvalidBitrate
    CommandFailureKind.BIND_FAILED -> CommandFailureBindFailed(detail)
    CommandFailureKind.WRITE_FAILED -> CommandFailureWriteFailed(detail)
    CommandFailureKind.UNSUPPORTED_BY_BACKEND -> CommandFailureUnsupportedByBackend
    CommandFailureKind.UNKNOWN_LINK -> CommandFailureUnknownLink
    CommandFailureKind.LINK_NOT_ACTIVE -> CommandFailureLinkNotActive
    CommandFailureKind.ENTROPY_UNAVAILABLE -> CommandFailureEntropyUnavailable
    CommandFailureKind.NOT_LINK_INITIATOR -> CommandFailureNotLinkInitiator
    CommandFailureKind.IDENTITY_NOT_HELD -> CommandFailureIdentityNotHeld
    CommandFailureKind.UNKNOWN_REQUEST_HANDLER -> CommandFailureUnknownRequestHandler
    CommandFailureKind.REQUEST_POLICY_NOT_ALLOW_LIST -> CommandFailureRequestPolicyNotAllowList
    CommandFailureKind.REQUEST_ALLOW_LIST_FULL -> CommandFailureRequestAllowListFull
    CommandFailureKind.LINK_BUSY -> CommandFailureLinkBusy
    CommandFailureKind.RESOURCE_TABLE_FULL -> CommandFailureResourceTableFull
    CommandFailureKind.RESOURCE_METADATA_TOO_LARGE -> CommandFailureResourceMetadataTooLarge
    CommandFailureKind.RESOURCE_REJECTED_BY_PEER -> CommandFailureResourceRejectedByPeer
    CommandFailureKind.RESOURCE_SEQUENCING_FAILED -> CommandFailureResourceSequencingFailed
    CommandFailureKind.RESOURCE_PREDECESSOR_FAILED -> CommandFailureResourcePredecessorFailed
    CommandFailureKind.CHANNEL_WINDOW_FULL -> CommandFailureChannelWindowFull
    CommandFailureKind.CHANNEL_UNTRACKABLE -> CommandFailureChannelUntrackable
    CommandFailureKind.INVALID_CHANNEL_MESSAGE_TYPE -> CommandFailureInvalidChannelMessageType
    CommandFailureKind.INVALID_CONFIGURATION -> CommandFailureInvalidConfiguration(detail)
    CommandFailureKind.RESOURCE_UPLOAD_CANCELLED -> CommandFailureResourceUploadCancelled
    CommandFailureKind.RESOURCE_EARLY_EOF -> CommandFailureResourceEarlyEof
    CommandFailureKind.RESOURCE_LENGTH_OVERRUN -> CommandFailureResourceLengthOverrun
    CommandFailureKind.PERMISSION_DENIED -> CommandFailurePermissionDenied(detail)
    CommandFailureKind.DEVICE_UNAVAILABLE -> CommandFailureDeviceUnavailable(detail)
    CommandFailureKind.CONNECT_FAILED -> CommandFailureConnectFailed(detail)
    CommandFailureKind.BACKEND_FAILED -> CommandFailureBackendFailed(detail)
    CommandFailureKind.RESPONSE_TOO_LARGE -> CommandFailureResponseTooLarge
}
