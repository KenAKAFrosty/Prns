package rs.reticulum.prns.expo

import java.util.concurrent.Executor
import java.util.concurrent.atomic.AtomicBoolean

/** Invoke on the lifecycle lane; asynchronous callbacks always return to it. */
internal fun <Value> prnsCompleteOnce(
  lifecycle: Executor,
  invoke: ((Result<Value>) -> Unit) -> Unit,
  completion: (Result<Value>) -> Unit,
) {
  val completed = AtomicBoolean(false)
  val deliver: (Result<Value>) -> Unit = { result ->
    if (completed.compareAndSet(false, true)) lifecycle.execute { completion(result) }
  }
  try { invoke(deliver) } catch (error: Exception) { deliver(Result.failure(error)) }
}

internal fun prnsStartDelegate(
  lifecycle: Executor,
  delegate: PrnsRuntimeDelegate,
  input: ByteArray,
  completion: (Result<PrnsRuntimeStart>) -> Unit,
) {
  if (delegate is PrnsAsyncRuntimeDelegate) {
    prnsCompleteOnce(lifecycle, { delegate.start(input, it) }, completion)
  } else {
    // Preserve the synchronous delegate's existing serial execution semantics.
    val result = runCatching { delegate.start(input) }
    completion(result)
  }
}

internal fun prnsStopDelegate(
  lifecycle: Executor,
  delegate: PrnsRuntimeDelegate,
  reset: Boolean,
  completion: (Result<PrnsRuntimeStop>) -> Unit,
) {
  if (delegate is PrnsAsyncRuntimeDelegate) {
    prnsCompleteOnce(lifecycle, { delegate.stop(reset, it) }, completion)
  } else {
    val result = runCatching { delegate.stop(reset) }
    completion(result)
  }
}

/** Public Stop response after native and SDK resource completion are combined. */
internal fun prnsStopResponse(delegate: PrnsRuntimeDelegate, result: Result<PrnsRuntimeStop>): Result<Any?> =
  result.mapCatching { outcome ->
    check(delegate !is PrnsAsyncRuntimeDelegate || outcome.stopped) {
      "Native or transport shutdown has not completed; try Stop again"
    }
    outcome.response
  }

/** Fence async native startup before touching restoration storage. */
internal fun prnsStopAndClearInput(
  lifecycle: Executor,
  delegate: PrnsRuntimeDelegate,
  reset: Boolean,
  completion: (Result<PrnsRuntimeStop>, Throwable?) -> Unit,
) {
  var clearFailure: Throwable? = null
  val stop = { prnsStopDelegate(lifecycle, delegate, reset) { completion(it, clearFailure) } }
  if (delegate is PrnsAsyncRuntimeDelegate) {
    // Its completion is always queued, including a synchronous callback/throw.
    // Slow or failed preference I/O therefore cannot postpone the native fence.
    stop()
    clearFailure = runCatching { delegate.clearInput() }.exceptionOrNull()
  } else {
    clearFailure = runCatching { delegate.clearInput() }.exceptionOrNull()
    stop()
  }
}

/** Every continuation rechecks the same service ticket, including after native settlement. */
internal fun prnsReplaceBluetooth(
  lifecycle: Executor,
  current: () -> Boolean,
  detach: ((Result<Unit>) -> Unit) -> Unit,
  prepare: () -> Unit,
  attach: ((Result<Unit>) -> Unit) -> Unit,
  admitted: () -> Unit = {},
  completion: (Result<Unit>) -> Unit,
) {
  fun requireCurrent() { check(current()) { "Bluetooth change was cancelled by Stop" } }
  try { requireCurrent(); admitted(); requireCurrent() }
  catch (error: Exception) { completion(Result.failure(error)); return }
  prnsCompleteOnce(lifecycle, detach) { detached ->
    val prepared = runCatching { requireCurrent(); detached.getOrThrow(); prepare(); requireCurrent() }
    if (prepared.isFailure) completion(prepared)
    else prnsCompleteOnce(lifecycle, attach) { attached ->
      completion(runCatching { requireCurrent(); attached.getOrThrow() })
    }
  }
}
