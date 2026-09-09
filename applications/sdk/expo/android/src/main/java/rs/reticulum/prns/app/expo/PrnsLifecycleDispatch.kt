package rs.reticulum.prns.app.expo

import java.util.concurrent.Executor

/** Refresh platform ownership before admitting an outbound command to the actor.
 *
 * Commands still run off the lifecycle queue: a long request must not prevent
 * Stop from cancelling it. This is admission ordering, never a request retry.
 */
internal fun prnsDispatchNativeCall(
  operation: String,
  lifecycle: Executor,
  calls: Executor,
  refresh: () -> Boolean,
  call: () -> Unit,
  failed: (Exception) -> Unit,
) {
  val dispatch = {
    calls.execute {
      try {
        call()
      } catch (error: Exception) {
        failed(error)
      }
    }
  }
  // Local reads (especially snapshot polling) must never drive radio recovery.
  // Approval/rejection/cancellation belong to an already-admitted operation and
  // must not restart its owner either.
  if (operation !in bluetoothPreflightOperations) {
    dispatch()
    return
  }
  lifecycle.execute {
    try {
      check(refresh()) { "The connection service has not finished recovering. Try Stop, then Start again." }
      dispatch()
    } catch (error: Exception) {
      failed(error)
    }
  }
}

private val bluetoothPreflightOperations = setOf(
  "initiatePairing", "describeTarget", "announceTarget", "announceLxmf",
  "sendDirectText", "retryLxmfMessage",
)
