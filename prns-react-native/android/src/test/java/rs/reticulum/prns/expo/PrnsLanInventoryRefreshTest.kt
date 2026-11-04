package rs.reticulum.prns.expo

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PrnsLanInventoryRefreshTest {
  @Test
  fun unchangedNetworkCallbacksPreserveExistingDiscovery() {
    val refresh = PrnsLanInventoryRefresh()
    repeat(3) {
      val decision = refresh.observe(PrnsLanNative.INVENTORY_UNCHANGED)
      assertFalse(decision.retryInventory)
      assertFalse(decision.restartDiscovery)
    }
  }

  @Test
  fun aChangedNetworkRetainsRetirementUntilAcknowledgedEvenWhenNextInventoryIsUnchanged() {
    val refresh = PrnsLanInventoryRefresh()
    assertTrue(refresh.observe(PrnsLanNative.INVENTORY_REPLACED).restartDiscovery)
    // A failed stop must not be forgotten when publishing the same snapshot.
    assertTrue(refresh.observe(PrnsLanNative.INVENTORY_UNCHANGED).restartDiscovery)
    refresh.retirementCompleted()
    assertFalse(refresh.observe(PrnsLanNative.INVENTORY_UNCHANGED).restartDiscovery)
  }

  @Test
  fun unavailableInventoryRetriesWithoutRetiringDiscoveryOrLosingPendingRetirement() {
    val refresh = PrnsLanInventoryRefresh()
    refresh.observe(PrnsLanNative.INVENTORY_REPLACED)
    val unavailable = refresh.observe(PrnsLanNative.INVENTORY_UNAVAILABLE)
    assertTrue(unavailable.retryInventory)
    assertFalse(unavailable.restartDiscovery)
    assertTrue(refresh.observe(PrnsLanNative.INVENTORY_UNCHANGED).restartDiscovery)
    assertTrue(refresh.observe(99).retryInventory)
  }
}
