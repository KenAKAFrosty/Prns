package rs.reticulum.prns.app.expo

import java.util.concurrent.Executor

/** Admit an outbound JS call only after platform recovery on the lifecycle queue.
 * The generated async domain call starts in JS after this promise resolves, so
 * it cannot hold up Stop. The SDK owns which calls need this admission step.
 */
internal fun prnsPrepareOutbound(
  lifecycle: Executor,
  refresh: () -> Boolean,
  prepared: () -> Unit,
  failed: (Exception) -> Unit,
) {
  lifecycle.execute {
    try {
      check(refresh()) { "The connection service has not finished recovering. Try Stop, then Start again." }
      prepared()
    } catch (error: Exception) {
      failed(error)
    }
  }
}
