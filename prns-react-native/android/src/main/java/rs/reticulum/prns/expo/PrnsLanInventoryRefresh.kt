package rs.reticulum.prns.expo

/** Preserve discovery across unrelated OS callbacks and retain unfinished retirement. */
internal class PrnsLanInventoryRefresh {
  private var restartPending = false

  data class Decision(val retryInventory: Boolean, val restartDiscovery: Boolean)

  fun observe(outcome: Int): Decision {
    when (outcome) {
      PrnsLanNative.INVENTORY_REPLACED -> restartPending = true
      PrnsLanNative.INVENTORY_UNCHANGED -> Unit
      else -> return Decision(retryInventory = true, restartDiscovery = false)
    }
    return Decision(retryInventory = false, restartDiscovery = restartPending)
  }

  fun retirementCompleted() { restartPending = false }
}
