package rs.reticulum.prns.expo

/** A callback target retains the exact native generation; late callbacks are rejected. */
class PrnsLanSessionNative(@JvmField val generation: Long) {
    external fun nativeWifiDiscoverySession(sessionId: Long, active: Boolean)

    external fun nativeWifiDiscoveryStarted(sessionId: Long)

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

 }

object PrnsLanNative {
    init { System.loadLibrary(PrnsNativeImage.name) }
    const val WIFI_DISCOVERY_INACTIVE = 0
    const val WIFI_DISCOVERY_SATELLITE = 1
    const val WIFI_DISCOVERY_CENTRAL = 2
    const val WIFI_RESOLVED_SERVICE_VISIBLE = 0
    const val WIFI_RESOLVED_SERVICE_REJECTED = 1
    const val WIFI_RESOLVED_SERVICE_AT_CAPACITY = 2
    const val WIFI_RESOLVED_SERVICE_UNAVAILABLE = 3
    const val INVENTORY_UNAVAILABLE = 0
    const val INVENTORY_UNCHANGED = 1
    const val INVENTORY_REPLACED = 2
    external fun nativeGeneration(): Long
    external fun nativeRevision(): Long
    external fun nativeWaitForChange(observed: Long, timeout: Long): Long
    external fun nativeWake()
    external fun nativeRetiring(generation: Long): Boolean
    external fun nativeStopped(generation: Long)
    external fun nativeNetworkPermission(generation: Long, allowed: Boolean)
    external fun nativeDetail(generation: Long, detail: String?)
    external fun nativeInventory(generation: Long, names: Array<String>, indices: IntArray, addresses: Array<ByteArray>, prefixes: IntArray, addressIndices: IntArray): Int
}
