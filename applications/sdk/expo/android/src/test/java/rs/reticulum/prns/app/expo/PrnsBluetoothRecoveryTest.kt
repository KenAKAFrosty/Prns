package rs.reticulum.prns.app.expo

import java.util.concurrent.Executor
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PrnsBluetoothRecoveryTest {
  private class QueueExecutor : Executor {
    val queued = ArrayDeque<Runnable>()
    override fun execute(command: Runnable) { queued.addLast(command) }
    fun next() { queued.removeFirst().run() }
  }

  @Test
  fun firstListenerAndRepeatedPublicationDoNotRestartTheNode() {
    val generation = PrnsBluetoothGeneration()
    assertFalse(generation.restartBeforeReplacement(engineRunning = true))
    val listener = generation.beginListener()
    assertFalse(generation.publish(listener, 131))
    assertFalse(generation.publish(listener, 131))
    assertTrue(generation.restartBeforeReplacement(engineRunning = true))
  }

  @Test
  fun platformRadioRecoveryCannotReopenTheRetiredListener() {
    val old = PrnsBluetoothRadioCycle()
    assertTrue(old.observeAvailable(true))
    assertFalse(old.observeAvailable(false))
    assertFalse(old.observeAvailable(true))
    assertFalse(old.observeAvailable(true)) // A foreground refresh is not a new owner.

    // A service-owned replacement can recover even with no Activity involved.
    assertTrue(PrnsBluetoothRadioCycle().observeAvailable(true))
  }

  @Test
  fun radioOnRestartsBeforeOneListenerAndAdmitsCheckOnlyAfterwards() {
    val lifecycle = QueueExecutor()
    val calls = QueueExecutor()
    val events = mutableListOf<String>()
    val generation = PrnsBluetoothGeneration()
    val oldListener = generation.beginListener()
    assertFalse(generation.publish(oldListener, 131))
    val radio = PrnsBluetoothRadioCycle()
    assertTrue(radio.observeAvailable(true))
    assertFalse(radio.observeAvailable(false))
    assertFalse(radio.observeAvailable(true)) // No temporary listener or scan.
    var currentListener: Long? = null
    var checks = 0
    val refresh = {
      if (currentListener == null) {
        if (generation.restartBeforeReplacement(engineRunning = true)) {
          events.add("stop native")
          generation.retireListener()
          generation.nativeStopped()
        }
        currentListener = generation.beginListener()
        events.add("start listener")
        events.add("start native")
        lifecycle.execute {
          // The delayed publication from this fresh owner must be a no-op.
          assertFalse(generation.publish(currentListener!!, 133))
          events.add("published")
        }
      }
      true
    }
    lifecycle.execute { refresh() } // Radio-on broadcast while the app is backgrounded.
    prnsDispatchNativeCall("describeTarget", lifecycle, calls, refresh, {
      checks++
      events.add("check")
    }, { throw AssertionError(it) })

    lifecycle.next()
    assertTrue(calls.queued.isEmpty())
    lifecycle.next() // Command preflight sees the already-created owner.
    calls.next() // Exercise Check before the delayed PSM callback, as on Galaxy.
    lifecycle.next()
    assertFalse(generation.publish(oldListener, 131)) // Stale callback cannot restart.
    refresh() // Foreground resume is unchanged, not another generation.

    assertEquals(listOf("stop native", "start listener", "start native", "check", "published"), events)
    assertEquals(1, checks)
  }

  @Test
  fun failedDrainDoesNotForgetTheOldNativePsm() {
    val generation = PrnsBluetoothGeneration()
    val oldListener = generation.beginListener()
    generation.publish(oldListener, 131)
    generation.retireListener()
    assertFalse(generation.publish(oldListener, 132))
    // nativeStopped is deliberately absent: replacement still requires a drain.
    assertTrue(generation.restartBeforeReplacement(engineRunning = true))
    assertFalse(generation.restartBeforeReplacement(engineRunning = false))
  }

  @Test
  fun anUnexpectedInPlacePsmChangeStillRequiresRecovery() {
    val generation = PrnsBluetoothGeneration()
    val listener = generation.beginListener()
    assertFalse(generation.publish(listener, 131))
    assertTrue(generation.publish(listener, 132))
  }

  @Test
  fun lifecyclePreflightRunsBeforeCallAndDoesNotHoldUpStop() {
    val lifecycle = QueueExecutor()
    val calls = QueueExecutor()
    val events = mutableListOf<String>()
    prnsDispatchNativeCall("describeTarget", lifecycle, calls,
      { events.add("refresh") },
      { events.add("call") },
      { throw AssertionError(it) },
    )
    lifecycle.execute { events.add("stop") }

    assertTrue(calls.queued.isEmpty())
    lifecycle.next()
    lifecycle.next() // Stop is not waiting for the native command worker.
    calls.next()
    assertEquals(listOf("refresh", "stop", "call"), events)
  }

  @Test
  fun failedPreflightDoesNotAdmitOrReplayTheCommand() {
    val lifecycle = QueueExecutor()
    val calls = QueueExecutor()
    var failures = 0
    prnsDispatchNativeCall("describeTarget", lifecycle, calls,
      { throw IllegalStateException("drain failed") },
      { throw AssertionError("command should not be admitted") },
      { failures++ },
    )
    lifecycle.next()
    assertEquals(1, failures)
    assertTrue(calls.queued.isEmpty())
    assertTrue(lifecycle.queued.isEmpty())
  }

  @Test
  fun failedNativeCallIsReportedOnceWithoutReplay() {
    val lifecycle = QueueExecutor()
    val calls = QueueExecutor()
    var attempts = 0
    var failures = 0
    prnsDispatchNativeCall("describeTarget", lifecycle, calls, { true }, {
      attempts++
      throw IllegalStateException("request failed")
    }, { failures++ })
    lifecycle.next()
    calls.next()
    assertEquals(1, attempts)
    assertEquals(1, failures)
    assertTrue(calls.queued.isEmpty())
    assertTrue(lifecycle.queued.isEmpty())
  }

  @Test
  fun replacementDrainsEvenBeforeTheFirstPsmCallbackArrives() {
    val generation = PrnsBluetoothGeneration()
    val oldListener = generation.beginListener()
    // Android published to Rust, but its service callback is still queued.
    assertTrue(generation.restartBeforeReplacement(engineRunning = true))
    generation.retireListener()
    generation.nativeStopped()
    val replacement = generation.beginListener()
    assertFalse(generation.publish(oldListener, 131))
    assertFalse(generation.publish(replacement, 133))
  }

  @Test
  fun localReadsAndInFlightOperationActionsNeverTriggerRecovery() {
    val lifecycle = QueueExecutor()
    val calls = QueueExecutor()
    val operations = listOf(
      "snapshot", "inspectIdentity", "listContacts", "listLxmfPeers", "listLxmfMessages",
      "approvePairing", "rejectPairing", "cancelLxmfMessage", "createManualContact",
    )
    val called = mutableListOf<String>()
    for (operation in operations) {
      prnsDispatchNativeCall(operation, lifecycle, calls,
        { throw AssertionError("$operation must not recover the radio") },
        { called.add(operation) },
        { throw AssertionError(it) },
      )
    }
    assertTrue(lifecycle.queued.isEmpty())
    while (calls.queued.isNotEmpty()) calls.next()
    assertEquals(operations, called)
  }

  @Test
  fun retainedOwnerAfterUnsuccessfulRecoveryCannotAdmitACommand() {
    val lifecycle = QueueExecutor()
    val calls = QueueExecutor()
    var failures = 0
    prnsDispatchNativeCall("describeTarget", lifecycle, calls,
      { false }, // Service reports an incomplete drain without throwing.
      { throw AssertionError("must not enter the retained native owner") },
      { failures++ },
    )
    lifecycle.next()
    assertEquals(1, failures)
    assertTrue(calls.queued.isEmpty())
    assertTrue(lifecycle.queued.isEmpty())
  }
}
