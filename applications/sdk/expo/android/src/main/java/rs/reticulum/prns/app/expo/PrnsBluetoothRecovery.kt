package rs.reticulum.prns.app.expo

/** A platform-unavailable cycle belongs to a retired listener, not an auto-retry.
 * The service drains and replaces that owner before Android publishes a new PSM.
 */
internal class PrnsBluetoothRadioCycle {
  private var unavailable = false

  fun observeAvailable(available: Boolean): Boolean {
    if (!available) unavailable = true
    return !unavailable
  }
}

/** PSM publication belongs to exactly one service-owned native generation. */
internal class PrnsBluetoothGeneration {
  private var listener = 0L
  private var listenerAttempted = false
  private var publishedPsm: Int? = null

  fun beginListener(): Long {
    listenerAttempted = true
    return ++listener
  }

  fun retireListener() { listener++ }

  fun isCurrent(candidate: Long): Boolean = candidate == listener

  fun restartBeforeReplacement(engineRunning: Boolean): Boolean =
    // Native publication precedes its queued service callback. An attempted
    // listener can already have supplied the native PSM even before publish().
    engineRunning && listenerAttempted

  /** False for a fresh, unchanged, or already retired listener publication. */
  fun publish(candidate: Long, psm: Int): Boolean {
    if (!isCurrent(candidate)) return false
    val previous = publishedPsm
    publishedPsm = psm
    return previous != null && previous != psm
  }

  fun nativeStopped() {
    listenerAttempted = false
    publishedPsm = null
  }
}
