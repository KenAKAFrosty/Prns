package org.personal.hopspot

import java.nio.ByteBuffer

object NativeBridge {
    const val BLE_RADIO_ENABLED = 0x01
    const val BLE_RADIO_ADVERTISING = 0x02
    const val BLE_RADIO_SCANNING = 0x04

    init {
        System.loadLibrary("personal_hopspot_android")
    }

    external fun nativeInit(): Long

    external fun nativeStartEngine(storageDir: String): Int

    external fun nativeStopEngine(): Int

    external fun nativeEngineState(): Int

    external fun nativeEngineLastFailure(): Int

    external fun nativeEngineLastFailureName(): String?

    private external fun nativeInputShortPressCode(): Int

    private external fun nativeInputLongPressCode(): Int

    private external fun nativeActionNoneCode(): Int

    private external fun nativeActionAnnounceCode(): Int

    private external fun nativeActionCopySharedInstanceConfigCode(): Int

    private external fun nativeEngineStoppedCode(): Int

    private external fun nativeEngineStartingCode(): Int

    private external fun nativeEngineRunningCode(): Int

    private external fun nativeEngineFailedCode(): Int

    private external fun nativePanelWidth(): Int

    private external fun nativePanelHeight(): Int

    private external fun nativeRgbaBytes(): Int

    private external fun nativeRenderIntervalMillis(): Long

    val INPUT_SHORT_PRESS = nativeInputShortPressCode()
    val INPUT_LONG_PRESS = nativeInputLongPressCode()
    val ACTION_NONE = nativeActionNoneCode()
    val ACTION_ANNOUNCE = nativeActionAnnounceCode()
    val ACTION_COPY_SHARED_INSTANCE_CONFIG = nativeActionCopySharedInstanceConfigCode()
    val ENGINE_STOPPED = nativeEngineStoppedCode()
    val ENGINE_STARTING = nativeEngineStartingCode()
    val ENGINE_RUNNING = nativeEngineRunningCode()
    val ENGINE_FAILED = nativeEngineFailedCode()
    val PANEL_WIDTH = nativePanelWidth()
    val PANEL_HEIGHT = nativePanelHeight()
    val RGBA_BYTES = nativeRgbaBytes()
    val RENDER_INTERVAL_MILLIS = nativeRenderIntervalMillis()

    external fun nativeFree(handle: Long)

    external fun nativePostInput(handle: Long, code: Int): Int

    external fun nativeAnnounce()

    external fun nativeRuntimeHealth(): LongArray?

    external fun nativePersistenceHealth(): LongArray?

    external fun nativeRpcKeyHex(): String?

    external fun nativeSidebandJoinConfig(): String?

    external fun nativeNodeIdentityHashHex(): String?

    external fun nativeBleIdentityHex(): String?

    external fun nativeDeliveryDestinationHex(): String?

    external fun nativeNodePageDestinationHex(): String?

    external fun nativeWifiAwareFailureReason(): String?

    external fun nativeWifiDirectFailureReason(): String?

    external fun nativeRender(handle: Long, buffer: ByteBuffer)

    external fun nativeSetBattery(handle: Long, percent: Int, externallyPowered: Boolean)

    external fun nativeUsbConnected(connected: Boolean)

    external fun nativeUsbAutoVendorId(): Int

    external fun nativeUsbAutoProductId(): Int

    external fun nativeUsbAccessoryManufacturer(): String

    external fun nativeUsbAccessoryModel(): String

    external fun nativeUsbAccessoryDescription(): String

    external fun nativeUsbAccessoryVersion(): String

    external fun nativeUsbAccessoryUri(): String

    external fun nativeUsbAccessorySerial(): String

    external fun nativeUsbRx(buffer: ByteBuffer, len: Int)

    external fun nativeUsbTx(buffer: ByteBuffer): Int

    const val WIFI_DISCOVERY_INACTIVE = 0
    const val WIFI_DISCOVERY_SATELLITE = 1
    const val WIFI_DISCOVERY_CENTRAL = 2
    const val WIFI_RESOLVED_SERVICE_VISIBLE = 0
    const val WIFI_RESOLVED_SERVICE_REJECTED = 1
    const val WIFI_RESOLVED_SERVICE_AT_CAPACITY = 2
    const val WIFI_RESOLVED_SERVICE_UNAVAILABLE = 3

    external fun nativeWifiTcpServicePort(): Int

    external fun nativeWifiUdpServicePort(): Int

    external fun nativeWifiTcpServiceType(): String

    external fun nativeWifiUdpServiceType(): String

    external fun nativeWifiTxtVersionKey(): String

    external fun nativeWifiTxtVersionValue(): String

    external fun nativeWifiServiceCapacity(): Int

    external fun nativeWifiCandidateCapacity(): Int

    external fun nativeWifiResolvedCandidateInputCapacity(): Int

    external fun nativeWifiDiscoveryParticipation(): Int

    external fun nativeWifiWorkGeneration(): Long

    external fun nativeWifiWaitForWork(observedGeneration: Long, timeoutMillis: Long): Long

    external fun nativeWifiWakeDiscoveryPump()

    external fun nativeWifiTcpPublicationName(): String?

    external fun nativeWifiUdpPublicationName(): String?

    external fun nativeWifiEndPublicationSession()

    external fun nativeWifiRegistered(serviceType: String, serviceInstance: String)

    external fun nativeWifiResolved(
        serviceType: String,
        serviceInstance: String,
        addresses: Array<ByteArray>,
        scopeIds: IntArray,
        port: Int,
        version: String?,
    ): Int

    external fun nativeWifiLost(serviceType: String, serviceInstance: String)

    external fun nativeBleSetPsm(psm: Int)

    external fun nativeBleDesiredState(): Int

    external fun nativeBlePeerCapacity(): Int

    external fun nativeBleWorkGeneration(): Long

    external fun nativeBleWaitForWork(observed: Long, timeoutMillis: Long): Long

    external fun nativeBleWakePumps()

    external fun nativeBleIdentity(buffer: ByteBuffer): Int

    external fun nativeBleGroupTag(buffer: ByteBuffer): Int

    external fun nativeBleDiscoveryGroup(): String?

    external fun nativeBleSetDiscoveryGroup(groupId: String): Boolean

    external fun nativeBleCycleDiscoveryGroup(): String?

    external fun nativeBleSighting(address: ByteBuffer, rssi: Int)

    external fun nativeBleDialFailed(address: ByteBuffer): Boolean

    external fun nativeBleLinkUp(connId: Int, address: ByteBuffer, rssi: Int, dialed: Boolean): Boolean

    external fun nativeBleColumbaLinkUp(
        connId: Int,
        address: ByteBuffer,
        rssi: Int,
        dialed: Boolean,
        peerIdentity: ByteBuffer,
    ): Boolean

    external fun nativeBleControlIn(connId: Int, buffer: ByteBuffer, len: Int): Int

    external fun nativeBleControlOut(connId: Int, buffer: ByteBuffer): Int

    external fun nativeBleCommitControlOut(connId: Int): Boolean

    external fun nativeBleL2capIn(connId: Int, buffer: ByteBuffer, len: Int): Boolean

    external fun nativeBleL2capOut(connId: Int, buffer: ByteBuffer): Int

    external fun nativeBleDataIn(connId: Int, buffer: ByteBuffer, len: Int): Int

    external fun nativeBleDataOut(connId: Int, buffer: ByteBuffer): Int

    external fun nativeBleCommitDataOut(connId: Int): Boolean

    external fun nativeBleL2capUp(connId: Int)

    external fun nativeBleDisconnected(connId: Int)

    external fun nativeBleNextClose(): Int

    external fun nativeBleNextDial(buffer: ByteBuffer): Boolean

    external fun nativeBleNextL2capOpen(buffer: ByteBuffer): Boolean

    const val BLE_INGRESS_ACCEPTED = 0
    const val BLE_INGRESS_FULL = 1
    const val BLE_INGRESS_CLOSED = 2

    const val WIFI_DIRECT_AVAILABLE = 0
    const val WIFI_DIRECT_DISABLED = 1
    const val WIFI_DIRECT_NO_PERMISSION = 2
    const val WIFI_DIRECT_EXPERIMENTAL_DISABLED = 3

    external fun nativeWifiDirectServiceType(): String

    external fun nativeWifiDirectDeviceMarker(): String

    external fun nativeWifiDirectNativeServiceInstance(): String

    external fun nativeWifiDirectSupplicantServiceInstance(): String

    external fun nativeWifiDirectSighting(
        address: ByteBuffer,
        peerIsSupplicant: Boolean,
        peerNameHash: Int,
    )

    external fun nativeWifiDirectSetLocalNameHash(hash: Int)

    external fun nativeWifiDirectPeerGone(address: ByteBuffer)

    external fun nativeWifiDirectInvitation(address: ByteBuffer)

    external fun nativeWifiDirectGroupFormed(isOwner: Boolean, ownerAddress: ByteBuffer)

    external fun nativeWifiDirectFormationFailed()

    external fun nativeWifiDirectGroupLost()

    external fun nativeWifiDirectAvailability(code: Int)

    external fun nativeWifiDirectDesiredDiscovery(): Boolean

    external fun nativeWifiDirectTakeFormationRequest(): ByteArray?

    external fun nativeWifiDirectTakeRemoveGroup(): Boolean

    const val WIFI_AWARE_AVAILABLE = 0
    const val WIFI_AWARE_DISABLED = 1
    const val WIFI_AWARE_NO_PERMISSION = 2

    external fun nativeWifiAwareServiceName(): String

    external fun nativeWifiAwarePassphrase(): String

    external fun nativeWifiAwareRendezvousPort(): Int

    external fun nativeWifiAwareLocalToken(): Int

    external fun nativeWifiAwarePeerDiscovered(peer: Int)

    external fun nativeWifiAwareNdpRequested(peer: Int)

    external fun nativeWifiAwareDataPathUp(
        peer: Int,
        isInitiator: Boolean,
        address: ByteBuffer,
        scope: Int,
    )

    external fun nativeWifiAwareDataPathDown(peer: Int, isInitiator: Boolean)

    external fun nativeWifiAwareNdpFailed(peer: Int, isInitiator: Boolean)

    external fun nativeWifiAwareAvailability(code: Int)

    external fun nativeWifiAwareDesiredDiscovery(): Boolean

    external fun nativeWifiAwareTakeRequest(): Long

    external fun nativeWifiAwareTakeAbandon(): Long

    fun runtimeHealth(): PrnsRuntimeHealth =
        PrnsRuntimeHealth.fromNative(nativeRuntimeHealth())

    fun persistenceHealth(): PrnsPersistenceHealth? =
        PrnsPersistenceHealth.fromNative(nativePersistenceHealth())

    fun engineState(): PrnsEngineState =
        PrnsEngineState.fromNative(nativeEngineState())

    fun engineFailure(): PrnsEngineFailure =
        PrnsEngineFailure(
            code = nativeEngineLastFailure(),
            name = nativeEngineLastFailureName() ?: "invalid",
        )
}

enum class PrnsEngineState(
    val wireName: String,
) {
    STOPPED("stopped"),
    STARTING("starting"),
    RUNNING("running"),
    FAILED("failed");

    companion object {
        fun fromNative(code: Int): PrnsEngineState =
            when (code) {
                NativeBridge.ENGINE_STOPPED -> STOPPED
                NativeBridge.ENGINE_STARTING -> STARTING
                NativeBridge.ENGINE_RUNNING -> RUNNING
                NativeBridge.ENGINE_FAILED -> FAILED
                else -> throw IllegalArgumentException("unknown native engine state $code")
            }
    }
}

data class PrnsEngineFailure(
    val code: Int,
    val name: String,
)

data class PrnsPersistenceHealth(
    val restoredRouteCount: Int,
    val restoredDestinationIdentityCount: Int,
    val restoredTunnelCount: Int,
    val restoredRatchetCount: Int,
    val refusedRestoreCount: Int,
    val droppedRestoreCount: Int,
    val successfulFlushCount: Long,
) {
    companion object {
        private const val FIELD_COUNT = 7

        fun fromNative(values: LongArray?): PrnsPersistenceHealth? {
            if (values == null || values.size < FIELD_COUNT) {
                return null
            }
            return PrnsPersistenceHealth(
                restoredRouteCount = values[0].toNonNegativeInt(),
                restoredDestinationIdentityCount = values[1].toNonNegativeInt(),
                restoredTunnelCount = values[2].toNonNegativeInt(),
                restoredRatchetCount = values[3].toNonNegativeInt(),
                refusedRestoreCount = values[4].toNonNegativeInt(),
                droppedRestoreCount = values[5].toNonNegativeInt(),
                successfulFlushCount = values[6].coerceAtLeast(0),
            )
        }

        private fun Long.toNonNegativeInt(): Int =
            coerceIn(0, Int.MAX_VALUE.toLong()).toInt()
    }
}

data class PrnsRuntimeHealth(
    val runtimeUptimeMs: Long,
    val interfaceCount: Int,
    val onlineInterfaceCount: Int,
    val localClientCount: Int,
    val routeCount: Int,
    val linkCount: Int,
    val transportedLinkCount: Int,
    val rxBytes: Long,
    val txBytes: Long,
    val rxBps: Long,
    val txBps: Long,
) {
    companion object {
        private const val FIELD_COUNT = 11

        fun fromNative(values: LongArray?): PrnsRuntimeHealth {
            if (values == null || values.size < FIELD_COUNT) {
                return PrnsRuntimeHealth(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
            }
            return PrnsRuntimeHealth(
                runtimeUptimeMs = values[0].coerceAtLeast(0),
                interfaceCount = values[1].toNonNegativeInt(),
                onlineInterfaceCount = values[2].toNonNegativeInt(),
                localClientCount = values[3].toNonNegativeInt(),
                routeCount = values[4].toNonNegativeInt(),
                linkCount = values[5].toNonNegativeInt(),
                transportedLinkCount = values[6].toNonNegativeInt(),
                rxBytes = values[7].coerceAtLeast(0),
                txBytes = values[8].coerceAtLeast(0),
                rxBps = values[9].coerceAtLeast(0),
                txBps = values[10].coerceAtLeast(0),
            )
        }

        private fun Long.toNonNegativeInt(): Int =
            coerceIn(0, Int.MAX_VALUE.toLong()).toInt()
    }
}
