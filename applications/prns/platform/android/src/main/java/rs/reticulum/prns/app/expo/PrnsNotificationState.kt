package rs.reticulum.prns.app.expo

/** Pure presentation policy; reading status never asks for permission. */
internal object PrnsNotificationState {
  fun status(
    permissionRequired: Boolean,
    permissionGranted: Boolean,
    asked: Boolean,
    blocked: Boolean,
    appEnabled: Boolean,
    channelEnabled: Boolean,
  ): String = when {
    permissionRequired && !permissionGranted -> when {
      blocked -> "blocked"
      asked -> "denied"
      else -> "notRequested"
    }
    !appEnabled || !channelEnabled -> "blocked"
    else -> "enabled"
  }
}
