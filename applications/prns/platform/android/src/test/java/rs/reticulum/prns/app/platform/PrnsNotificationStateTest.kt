package rs.reticulum.prns.app.platform

import org.junit.Assert.assertEquals
import org.junit.Test

class PrnsNotificationStateTest {
  @Test fun permissionAndChannelStatesStayDistinct() {
    assertEquals("notRequested", PrnsNotificationState.status(true, false, false, false, false, true))
    assertEquals("denied", PrnsNotificationState.status(true, false, true, false, false, true))
    assertEquals("blocked", PrnsNotificationState.status(true, false, true, true, false, true))
    assertEquals("blocked", PrnsNotificationState.status(true, true, true, false, true, false))
    assertEquals("enabled", PrnsNotificationState.status(true, true, true, false, true, true))
    assertEquals("enabled", PrnsNotificationState.status(false, false, false, false, true, true))
  }
}
