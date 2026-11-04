package rs.reticulum.prns.app.platform

/** Reading this presentation state never requests OS permission. */
internal object PrnsNotificationState {
  fun status(required: Boolean, granted: Boolean, asked: Boolean, blocked: Boolean,
    appEnabled: Boolean, channelEnabled: Boolean): String = when {
    required && !granted -> when { blocked -> "blocked"; asked -> "denied"; else -> "notRequested" }
    !appEnabled || !channelEnabled -> "blocked"
    else -> "enabled"
  }
}
