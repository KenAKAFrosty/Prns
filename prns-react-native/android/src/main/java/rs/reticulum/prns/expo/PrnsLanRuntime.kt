package rs.reticulum.prns.expo

import android.content.Context
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Build
import java.net.NetworkInterface
import java.util.concurrent.atomic.AtomicBoolean

/** Native service ownership survives React reloads and native node replacement. */
class PrnsLanRuntime(context: Context) {
  private val application = context.applicationContext
  private val connectivity = application.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
  private val running = AtomicBoolean(false)
  private val refreshRequested = AtomicBoolean(true)
  private var worker: Thread? = null
  private val callback = object : ConnectivityManager.NetworkCallback() {
    override fun onAvailable(network: Network) = changed()
    override fun onLost(network: Network) = changed()
    override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) = changed()
    override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) = changed()
  }
  private fun changed() { refreshRequested.set(true); PrnsLanNative.nativeWake() }

  fun start() {
    if (!running.compareAndSet(false, true)) return
    try {
      connectivity?.registerNetworkCallback(NetworkRequest.Builder().addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN).build(), callback)
    } catch (_: SecurityException) { /* Observation failure cannot stop other transports. */ }
    worker = Thread(::run, "prns-lan-owner").also { it.start() }
  }
  fun refresh() { PrnsLanNative.nativeWake() }
  fun stop(): Boolean {
    running.set(false); PrnsLanNative.nativeWake()
    try { connectivity?.unregisterNetworkCallback(callback) } catch (_: RuntimeException) {}
    val current = worker
    if (current != null && current !== Thread.currentThread()) {
      try { current.join(6_000) } catch (_: InterruptedException) { Thread.currentThread().interrupt(); return false }
    }
    return current?.isAlive != true
  }

  private fun run() {
    var revision = PrnsLanNative.nativeRevision()
    var generation = 0L
    var discovery: PrnsLanDiscovery? = null
    var inventoryDetail: String? = null
    var inventoryRefresh = PrnsLanInventoryRefresh()
    while (running.get()) {
      val next = PrnsLanNative.nativeGeneration()
      if (generation != 0L && (next != generation || PrnsLanNative.nativeRetiring(generation))) {
        if (discovery?.stopAndJoin() != false) {
          PrnsLanNative.nativeStopped(generation)
          discovery = null; generation = 0
        }
      }
      if (generation == 0L && next != 0L && !PrnsLanNative.nativeRetiring(next)) {
        generation = next
        discovery = PrnsLanDiscovery(application, PrnsLanSessionNative(generation))
        inventoryDetail = null
        inventoryRefresh = PrnsLanInventoryRefresh()
        refreshRequested.set(true)
      }
      if (generation != 0L && !PrnsLanNative.nativeRetiring(generation)) {
        val allowed = hasLocalNetworkPermission(application)
        PrnsLanNative.nativeNetworkPermission(generation, allowed)
        if (allowed) {
          if (refreshRequested.getAndSet(false)) {
            val publication = publishInventory(generation)
            inventoryDetail = publication.detail
            val decision = inventoryRefresh.observe(publication.outcome)
            if (decision.retryInventory) {
              refreshRequested.set(true)
              PrnsLanNative.nativeDetail(generation, inventoryDetail)
              revision = PrnsLanNative.nativeWaitForChange(revision, 1_000)
              continue
            }
            if (decision.restartDiscovery && discovery?.stopAndJoin() == false) {
              refreshRequested.set(true)
              PrnsLanNative.nativeDetail(generation, "Previous local discovery is still stopping.")
              revision = PrnsLanNative.nativeWaitForChange(revision, 1_000)
              continue
            }
            if (decision.restartDiscovery) inventoryRefresh.retirementCompleted()
          }
          discovery?.start()
          PrnsLanNative.nativeDetail(generation, inventoryDetail)
        } else {
          discovery?.stopAndJoin()
          PrnsLanNative.nativeDetail(generation, "Allow local network access in app permissions to discover nearby nodes.")
        }
      }
      revision = PrnsLanNative.nativeWaitForChange(revision, 1_000)
    }
    if (generation != 0L && discovery?.stopAndJoin() != false) PrnsLanNative.nativeStopped(generation)
  }

  // allNetworks provides the initial inventory on supported Android 10+ devices;
  // callbacks invalidate this snapshot when networks or link addresses change.
  private data class InventoryPublication(val outcome: Int, val detail: String? = null)

  @Suppress("DEPRECATION")
  private fun publishInventory(generation: Long): InventoryPublication {
    val names = mutableListOf<String>(); val indices = mutableListOf<Int>()
    val addresses = mutableListOf<ByteArray>(); val prefixes = mutableListOf<Int>(); val addressIndices = mutableListOf<Int>()
    try {
      for (network in connectivity?.allNetworks.orEmpty()) {
        val capabilities = connectivity?.getNetworkCapabilities(network) ?: continue
        if (!capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) && !capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)) continue
        val properties = connectivity?.getLinkProperties(network) ?: continue
        val name = properties.interfaceName ?: continue
        val index = NetworkInterface.getByName(name)?.index ?: continue
        if (index <= 0 || names.contains(name) || names.size >= 8) continue
        names.add(name); indices.add(index)
        for (address in properties.linkAddresses.take(8)) {
          addresses.add(address.address.address); prefixes.add(address.prefixLength); addressIndices.add(index)
        }
      }
      val outcome = PrnsLanNative.nativeInventory(generation, names.toTypedArray(), indices.toIntArray(), addresses.toTypedArray(), prefixes.toIntArray(), addressIndices.toIntArray())
      return InventoryPublication(outcome, if (outcome == PrnsLanNative.INVENTORY_UNAVAILABLE) inventoryUnavailableDetail else null)
    } catch (_: RuntimeException) {
      return InventoryPublication(PrnsLanNative.INVENTORY_UNAVAILABLE, inventoryUnavailableDetail)
    } catch (_: java.net.SocketException) {
      return InventoryPublication(PrnsLanNative.INVENTORY_UNAVAILABLE, inventoryUnavailableDetail)
    }
  }

  companion object {
    private const val inventoryUnavailableDetail = "Local network details could not be refreshed. Discovery will retry."
    const val LOCAL_NETWORK_PERMISSION = "android.permission.ACCESS_LOCAL_NETWORK"
    fun requiresLocalNetworkPermission(context: Context) = Build.VERSION.SDK_INT >= 37 && context.applicationInfo.targetSdkVersion >= 37
    fun hasLocalNetworkPermission(context: Context) = !requiresLocalNetworkPermission(context) || context.checkSelfPermission(LOCAL_NETWORK_PERMISSION) == PackageManager.PERMISSION_GRANTED
  }
}
