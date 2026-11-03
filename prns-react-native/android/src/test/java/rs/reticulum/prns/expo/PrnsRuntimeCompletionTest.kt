package rs.reticulum.prns.expo

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import java.util.concurrent.Executor
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class PrnsRuntimeCompletionTest {
  private class QueueExecutor : Executor {
    val queued = ArrayDeque<Runnable>()
    override fun execute(command: Runnable) { queued.addLast(command) }
    fun next() { queued.removeFirst().run() }
  }

  @Test fun bluetoothReplacementSettlesDetachBeforePrepareAndAttachExactlyOnce() {
    val lane = QueueExecutor()
    val events = mutableListOf<String>()
    lateinit var detached: (Result<Unit>) -> Unit
    lateinit var attached: (Result<Unit>) -> Unit
    prnsReplaceBluetooth(lane, { true }, { detached = it; events.add("detach") },
      { events.add("prepare") }, { attached = it; events.add("attach") }) {
      it.getOrThrow(); events.add("complete")
    }
    assertEquals(listOf("detach"), events)
    detached(Result.success(Unit)); detached(Result.failure(IllegalStateException("duplicate")))
    lane.next()
    assertEquals(listOf("detach", "prepare", "attach"), events)
    attached(Result.success(Unit)); attached(Result.success(Unit)); lane.next()
    assertEquals(listOf("detach", "prepare", "attach", "complete"), events)
    assertTrue(lane.queued.isEmpty())
  }

  @Test fun stopDuringBluetoothDetachCannotReprepareOrAttach() {
    val lane = QueueExecutor()
    var current = true
    lateinit var detached: (Result<Unit>) -> Unit
    var result: Result<Unit>? = null
    prnsReplaceBluetooth(lane, { current }, { detached = it },
      { error("retired preparation") }, { error("retired attachment") }) { result = it }
    current = false
    detached(Result.success(Unit)); lane.next()
    assertTrue(result!!.isFailure)
  }

  @Test fun bluetoothDetachFailureIsPreservedWithoutPreparingReplacement() {
    val lane = QueueExecutor()
    val failure = IllegalStateException("incomplete drain")
    var result: Result<Unit>? = null
    prnsReplaceBluetooth(lane, { true }, { it(Result.failure(failure)) },
      { error("failed drain preparation") }, { error("failed drain attachment") }) { result = it }
    lane.next()
    assertSame(failure, result!!.exceptionOrNull())
  }

  @Test fun stopDuringBluetoothAttachCannotPublishSuccess() {
    val lane = QueueExecutor()
    var current = true
    lateinit var attached: (Result<Unit>) -> Unit
    var result: Result<Unit>? = null
    prnsReplaceBluetooth(lane, { current }, { it(Result.success(Unit)) }, {},
      { attached = it }) { result = it }
    lane.next(); current = false
    attached(Result.success(Unit)); lane.next()
    assertTrue(result!!.isFailure)
  }

  @Test fun bluetoothSettingsAreSavedOnlyAfterAdmissionAndFailedSaveKeepsAttachment() {
    val lane = QueueExecutor()
    var writes = 0
    var result: Result<Unit>? = null
    prnsReplaceBluetooth(lane, { false }, { error("retired detach") }, {}, {},
      admitted = { writes++ }) { result = it }
    assertEquals(0, writes)
    assertTrue(result!!.isFailure)
    val failure = IllegalStateException("disk write failed")
    prnsReplaceBluetooth(lane, { true }, { error("failed save detached") }, {}, {},
      admitted = { writes++; throw failure }) { result = it }
    assertEquals(1, writes)
    assertSame(failure, result!!.exceptionOrNull())
    assertTrue(lane.queued.isEmpty())
  }

  @Test fun stopDuringAdmittedSettingsWritePreventsDetachAndReplacement() {
    val lane = QueueExecutor()
    var current = true
    var result: Result<Unit>? = null
    prnsReplaceBluetooth(lane, { current }, { error("stopped detach") }, {}, {},
      admitted = { current = false }) { result = it }
    assertTrue(result!!.isFailure)
  }

  @Test fun idleStateRejectsPendingStartAndRetainedPhysicalOwnerOrStop() {
    val admission = PrnsRuntimeAdmission<Any>()
    assertTrue(admission.isIdle())
    val start = admission.enqueueStart()
    assertFalse(admission.isIdle())
    admission.completeStart(start)
    val owner = Any()
    admission.reserveOwner(owner)
    assertFalse(admission.isIdle())
    val stop = admission.beginStopOperation()
    assertFalse(admission.isIdle())
    admission.releaseOwner(owner, true)
    assertFalse(admission.isIdle())
    admission.completeStop(stop, true)
    assertTrue(admission.isIdle())
  }

  private abstract class Delegate : PrnsRuntimeDelegate {
    override val notificationId = 1
    override fun notification(service: Service, stop: PendingIntent): Notification = error("not used")
    override fun savedInput(): ByteArray? = null
    override fun saveInput(input: ByteArray) = Unit
    override fun clearInput() = Unit
  }

  private class AsyncDelegate : Delegate(), PrnsAsyncRuntimeDelegate {
    val starts = mutableListOf<(Result<PrnsRuntimeStart>) -> Unit>()
    val stops = mutableListOf<Pair<Boolean, (Result<PrnsRuntimeStop>) -> Unit>>()
    override fun start(input: ByteArray, completion: (Result<PrnsRuntimeStart>) -> Unit) { starts.add(completion) }
    override fun stop(reset: Boolean, completion: (Result<PrnsRuntimeStop>) -> Unit) { stops.add(reset to completion) }
  }

  @Test
  fun synchronousDelegateKeepsImmediateSerialCompletions() {
    val lifecycle = QueueExecutor()
    val events = mutableListOf<String>()
    val delegate = object : Delegate() {
      override fun start(input: ByteArray): PrnsRuntimeStart {
        events.add("start")
        return PrnsRuntimeStart("started", true)
      }
      override fun stop(reset: Boolean): PrnsRuntimeStop {
        events.add("stop:$reset")
        return PrnsRuntimeStop("stopped", true)
      }
    }
    lifecycle.execute {
      prnsStartDelegate(lifecycle, delegate, byteArrayOf()) { events.add(it.getOrThrow().response as String) }
      prnsStopDelegate(lifecycle, delegate, true) { events.add(it.getOrThrow().response as String) }
    }
    lifecycle.next()
    assertEquals(listOf("start", "started", "stop:true", "stopped"), events)
    assertTrue(lifecycle.queued.isEmpty())
  }

  @Test
  fun stopFencesHeldStartWithoutWaitingForReadinessAndLateReadyCannotSaveInput() {
    val lifecycle = QueueExecutor()
    val admission = PrnsRuntimeAdmission<Any>()
    val owner = Any()
    val delegate = AsyncDelegate()
    assertTrue(admission.reserveOwner(owner))
    val ticket = admission.enqueueStart()
    var saved = 0
    lifecycle.execute {
      prnsStartDelegate(lifecycle, delegate, byteArrayOf(1)) {
        if (admission.canStart(owner, ticket)) saved++
      }
    }
    lifecycle.next()
    assertEquals(1, delegate.starts.size)
    val stop = admission.beginStopOperation()
    lifecycle.execute {
      prnsStopDelegate(lifecycle, delegate, stop.reset) {
        assertTrue(admission.completeStop(stop, it.getOrThrow().stopped))
      }
    }
    lifecycle.next()
    assertEquals(1, delegate.stops.size) // Ready is still withheld.
    delegate.starts.single()(Result.success(PrnsRuntimeStart(null, true)))
    delegate.stops.single().second(Result.success(PrnsRuntimeStop(null, true)))
    lifecycle.next()
    lifecycle.next()
    assertEquals(0, saved)
    assertFalse(admission.isOwnerActive(owner))
  }

  @Test
  fun duplicateCallbacksAndThrowAfterCompletionDeliverExactlyOnceOnTheLane() {
    val lifecycle = QueueExecutor()
    var calls = 0
    val delegate = object : Delegate(), PrnsAsyncRuntimeDelegate {
      override fun start(input: ByteArray, completion: (Result<PrnsRuntimeStart>) -> Unit) {
        completion(Result.success(PrnsRuntimeStart("first", true)))
        completion(Result.failure(IllegalStateException("duplicate")))
        throw IllegalStateException("after completion")
      }
      override fun stop(reset: Boolean, completion: (Result<PrnsRuntimeStop>) -> Unit) {
        completion(Result.success(PrnsRuntimeStop("first", true)))
        completion(Result.success(PrnsRuntimeStop("second", false)))
        throw IllegalStateException("after completion")
      }
    }
    prnsStartDelegate(lifecycle, delegate, byteArrayOf()) {
      calls++
      assertEquals("first", it.getOrThrow().response)
    }
    prnsStopDelegate(lifecycle, delegate, false) {
      calls++
      assertEquals("first", it.getOrThrow().response)
    }
    assertEquals(0, calls)
    assertEquals(2, lifecycle.queued.size)
    lifecycle.next()
    lifecycle.next()
    assertEquals(2, calls)
    assertTrue(lifecycle.queued.isEmpty())
  }

  @Test
  fun thrownEntryPointUsesTheSameCompletionAndAsyncSyncEntryPointsReject() {
    val lifecycle = QueueExecutor()
    val failure = IllegalStateException("native failed")
    val delegate = object : Delegate(), PrnsAsyncRuntimeDelegate {
      override fun start(input: ByteArray, completion: (Result<PrnsRuntimeStart>) -> Unit) { throw failure }
      override fun stop(reset: Boolean, completion: (Result<PrnsRuntimeStop>) -> Unit) { throw failure }
    }
    var failures = 0
    prnsStartDelegate(lifecycle, delegate, byteArrayOf()) { assertSame(failure, it.exceptionOrNull()); failures++ }
    prnsStopDelegate(lifecycle, delegate, false) { assertSame(failure, it.exceptionOrNull()); failures++ }
    lifecycle.next()
    lifecycle.next()
    assertEquals(2, failures)
    assertTrue(runCatching { delegate.start(byteArrayOf()) }.exceptionOrNull() is UnsupportedOperationException)
    assertTrue(runCatching { delegate.stop(false) }.exceptionOrNull() is UnsupportedOperationException)
  }

  @Test
  fun failedDrainRetainsExactOwnerAndLatestStopAcrossRetries() {
    val lifecycle = QueueExecutor()
    val admission = PrnsRuntimeAdmission<Any>()
    val owner = Any()
    val delegate = AsyncDelegate()
    admission.reserveOwner(owner)
    val first = admission.beginStopOperation()
    prnsStopDelegate(lifecycle, delegate, false) { admission.completeStop(first, it.getOrThrow().stopped) }
    delegate.stops[0].second(Result.success(PrnsRuntimeStop(null, false)))
    lifecycle.next()
    assertTrue(admission.isStopCurrent(first))
    assertSame(owner, admission.currentOwner())
    assertFalse(admission.reserveOwner(Any()))
    val retry = admission.beginStopOperation()
    prnsStopDelegate(lifecycle, delegate, false) {
      if (admission.completeStop(retry, it.getOrThrow().stopped)) admission.releaseOwner(owner, true)
    }
    delegate.stops[0].second(Result.success(PrnsRuntimeStop(null, true))) // Duplicate old callback.
    assertTrue(lifecycle.queued.isEmpty())
    delegate.stops[1].second(Result.success(PrnsRuntimeStop(null, true)))
    lifecycle.next()
    val replacement = Any()
    assertTrue(admission.reserveOwner(replacement))
    assertFalse(admission.releaseOwner(owner, true))
    assertSame(replacement, admission.currentOwner())
  }

  @Test
  fun destructionDuringReconnectPreventsItsDelayedRestart() {
    val lifecycle = QueueExecutor()
    val admission = PrnsRuntimeAdmission<Any>()
    val owner = Any()
    val delegate = AsyncDelegate()
    admission.reserveOwner(owner)
    val reconnect = admission.enqueueStart()
    prnsStopDelegate(lifecycle, delegate, false) {
      if (admission.canStart(owner, reconnect)) {
        prnsStartDelegate(lifecycle, delegate, byteArrayOf()) { error("retired owner restarted") }
      }
    }
    admission.retireOwner(owner)
    val destruction = admission.beginStopOperation()
    prnsStopDelegate(lifecycle, delegate, false) { admission.completeStop(destruction, it.getOrThrow().stopped) }
    delegate.stops[0].second(Result.success(PrnsRuntimeStop(null, true)))
    lifecycle.next()
    assertTrue(delegate.starts.isEmpty())
    delegate.stops[1].second(Result.success(PrnsRuntimeStop(null, true)))
    lifecycle.next()
    assertFalse(admission.isOwnerActive(owner))
  }

  @Test
  fun asyncPublicStopCannotReturnNativeSuccessWhileTransportIsStillDraining() {
    val delegate = AsyncDelegate()
    val nativeSuccess = "native-only response"
    val incomplete = prnsStopResponse(delegate, Result.success(PrnsRuntimeStop(nativeSuccess, false)))
    assertTrue(incomplete.isFailure)
    assertTrue(incomplete.exceptionOrNull()?.message?.contains("shutdown has not completed") == true)
    assertEquals(nativeSuccess, prnsStopResponse(delegate, Result.success(PrnsRuntimeStop(nativeSuccess, true))).getOrThrow())
    val failure = IllegalStateException("native close failed")
    assertSame(failure, prnsStopResponse(delegate, Result.failure(failure)).exceptionOrNull())
  }

  @Test
  fun synchronousPublicStopRetainsItsOpaqueResponseContract() {
    val delegate = object : Delegate() {
      override fun start(input: ByteArray) = PrnsRuntimeStart(null, true)
      override fun stop(reset: Boolean) = PrnsRuntimeStop("existing response", false)
    }
    assertEquals("existing response", prnsStopResponse(delegate, Result.success(delegate.stop(false))).getOrThrow())
  }

  @Test
  fun racingForeignCallbacksDeliverOnlyOnceAndNeverRunCompletionOffLane() {
    val lifecycle = QueueExecutor()
    val delegate = AsyncDelegate()
    var calls = 0
    prnsStartDelegate(lifecycle, delegate, byteArrayOf()) { calls++ }
    val release = java.util.concurrent.CountDownLatch(1)
    val threads = (1..2).map {
      Thread {
        release.await()
        delegate.starts.single()(Result.success(PrnsRuntimeStart(null, true)))
      }.also { it.start() }
    }
    release.countDown()
    threads.forEach { it.join() }
    assertEquals(0, calls)
    assertEquals(1, lifecycle.queued.size)
    lifecycle.next()
    assertEquals(1, calls)
  }

  @Test
  fun asyncStopFencesBeforeInputStorageAndStillReportsItsFailureAfterNativeDrain() {
    val lifecycle = QueueExecutor()
    val calls = mutableListOf<String>()
    val inputFailure = IllegalStateException("preferences unavailable")
    val delegate = object : Delegate(), PrnsAsyncRuntimeDelegate {
      override fun start(input: ByteArray, completion: (Result<PrnsRuntimeStart>) -> Unit) = error("not used")
      override fun stop(reset: Boolean, completion: (Result<PrnsRuntimeStop>) -> Unit) {
        calls.add("native fence")
        completion(Result.success(PrnsRuntimeStop("native joined", true)))
      }
      override fun clearInput() {
        assertEquals(listOf("native fence"), calls)
        calls.add("clear input")
        throw inputFailure
      }
    }
    prnsStopAndClearInput(lifecycle, delegate, false) { result, clearFailure ->
      calls.add("complete")
      assertTrue(result.getOrThrow().stopped)
      assertSame(inputFailure, clearFailure)
    }
    assertEquals(listOf("native fence", "clear input"), calls)
    lifecycle.next()
    assertEquals(listOf("native fence", "clear input", "complete"), calls)
  }

  @Test
  fun stopRequestedAfterCompletionAdmissionCannotAcknowledgeBeforeSavedInputIsCleared() {
    val lifecycle = QueueExecutor()
    val admission = PrnsRuntimeAdmission<Any>()
    val owner = Any()
    admission.reserveOwner(owner)
    val ticket = admission.enqueueStart()
    val events = mutableListOf<String>()
    var saved: ByteArray? = null
    var ready: ((Result<PrnsRuntimeStart>) -> Unit)? = null
    val delegate = object : Delegate(), PrnsAsyncRuntimeDelegate {
      override fun start(input: ByteArray, completion: (Result<PrnsRuntimeStart>) -> Unit) { ready = completion }
      override fun stop(reset: Boolean, completion: (Result<PrnsRuntimeStop>) -> Unit) {
        assertFalse(Thread.holdsLock(admission))
        events.add("native fence")
        completion(Result.success(PrnsRuntimeStop(null, true)))
      }
      override fun saveInput(input: ByteArray) {
        assertFalse(Thread.holdsLock(admission))
        // Simulate Stop arriving after the completion's canStart check, but
        // before this admitted persistence operation has finished.
        val stop = admission.beginStopOperation()
        events.add("Stop requested")
        lifecycle.execute {
          prnsStopAndClearInput(lifecycle, this, stop.reset) { outcome, clearFailure ->
            assertEquals(null, clearFailure)
            assertTrue(admission.completeStop(stop, outcome.getOrThrow().stopped))
            assertTrue(prnsStopResponse(this, outcome).isSuccess)
            events.add("Stop acknowledged")
          }
        }
        saved = input
        events.add("saved")
      }
      override fun clearInput() {
        assertFalse(Thread.holdsLock(admission))
        saved = null
        events.add("cleared")
      }
    }
    prnsStartDelegate(lifecycle, delegate, byteArrayOf(1)) {
      if (admission.canStart(owner, ticket)) {
        events.add("ready admitted")
        delegate.saveInput(byteArrayOf(1))
      }
    }
    ready!!(Result.success(PrnsRuntimeStart(null, true)))
    lifecycle.next() // Admitted completion; Stop arrives while it persists.
    assertTrue(saved != null)
    assertEquals(listOf("ready admitted", "Stop requested", "saved"), events)
    lifecycle.next() // Stop fences and clears; completion remains queued.
    assertEquals(null, saved)
    assertFalse(events.contains("Stop acknowledged"))
    lifecycle.next()
    assertEquals(listOf("ready admitted", "Stop requested", "saved", "native fence", "cleared", "Stop acknowledged"), events)
    assertFalse(admission.canStart(owner, ticket))
  }
}
