package rs.reticulum.prns.app.expo

import org.junit.Assert.assertEquals
import org.junit.Test

class PrnsNotificationStateTest {
  @Test fun legacyAndroidNeedsNoRuntimeGrantButRespectsAppAndChannelSettings() {
    assertEquals("enabled", state(required = false, granted = false))
    assertEquals("blocked", state(required = false, granted = false, app = false))
    assertEquals("blocked", state(required = false, granted = false, channel = false))
  }

  @Test fun modernAndroidDistinguishesFirstRequestDenialAndSettingsRecovery() {
    assertEquals("notRequested", state(granted = false, app = false))
    assertEquals("denied", state(granted = false, asked = true, app = false))
    assertEquals("blocked", state(granted = false, asked = true, blocked = true, app = false))
  }

  @Test fun permissionAloneDoesNotPromiseAVisibleNotification() {
    assertEquals("blocked", state(app = false))
    assertEquals("blocked", state(channel = false))
    assertEquals("enabled", state())
  }

  @Test fun currentGrantOverridesAStoredDenial() {
    assertEquals("enabled", state(asked = true, blocked = true))
  }

  private fun state(
    required: Boolean = true,
    granted: Boolean = true,
    asked: Boolean = false,
    blocked: Boolean = false,
    app: Boolean = true,
    channel: Boolean = true,
  ): String = PrnsNotificationState.status(required, granted, asked, blocked, app, channel)
}
