package rs.reticulum.prns.app.platform

import org.junit.Assert.*
import org.junit.Test

class PrnsAppInputTest {
  @Test fun restorationAlwaysOpensWithoutChangingTransportChoice() {
    for (bluetooth in listOf(false, true)) {
      val initial = PrnsAppInput.from("create", bluetooth)
      assertEquals(initial, PrnsAppInput.decode(initial.encode()))
      assertEquals(PrnsAppInput(false, bluetooth), PrnsAppInput.decode(initial.forRestoration().encode()))
    }
  }
  @Test fun malformedSavedInputNeverBecomesCreate() {
    for (bytes in listOf(byteArrayOf(), byteArrayOf(1, 0), byteArrayOf(2, 0, 0),
      byteArrayOf(1, 2, 0), byteArrayOf(1, 0, 2), byteArrayOf(1, 0, 0, 0))) {
      try { PrnsAppInput.decode(bytes); fail("Malformed input was accepted") }
      catch (_: IllegalArgumentException) { }
    }
    try { PrnsAppInput.from("reset", false); fail("Unknown mode was accepted") }
    catch (_: IllegalArgumentException) { }
  }
}
