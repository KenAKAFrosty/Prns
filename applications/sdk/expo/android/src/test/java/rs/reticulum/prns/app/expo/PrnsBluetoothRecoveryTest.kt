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
  fun recoveryCompletesBeforeAdmissionAndDoesNotHoldUpStop() {
    val lifecycle = QueueExecutor()
    val calls = QueueExecutor()
    val events = mutableListOf<String>()
    prnsPrepareOutbound(lifecycle,
      { events.add("refresh"); true },
      { calls.execute { events.add("generated call") } },
      { throw AssertionError(it) },
    )
    lifecycle.execute { events.add("stop") }
    assertTrue(calls.queued.isEmpty())
    lifecycle.next()
    lifecycle.next()
    calls.next()
    assertEquals(listOf("refresh", "stop", "generated call"), events)
  }

  @Test
  fun incompleteRecoveryRejectsOnceWithoutAdmittingOrRetrying() {
    for (throws in listOf(false, true)) {
      val lifecycle = QueueExecutor()
      var failures = 0
      prnsPrepareOutbound(lifecycle,
        { if (throws) throw IllegalStateException("drain failed"); false },
        { throw AssertionError("must not admit while owner is retained") },
        { failures++ },
      )
      lifecycle.next()
      assertEquals(1, failures)
      assertTrue(lifecycle.queued.isEmpty())
    }
  }

  @Test
  fun freshPublicationCannotRestartARecoveredGenerationBeforeAdmission() {
    val lifecycle = QueueExecutor()
    val generation = PrnsBluetoothGeneration()
    val old = generation.beginListener()
    generation.publish(old, 131)
    val events = mutableListOf<String>()
    var replacement = 0L
    prnsPrepareOutbound(lifecycle, {
      assertTrue(generation.restartBeforeReplacement(engineRunning = true))
      events.add("stop native")
      generation.retireListener()
      generation.nativeStopped()
      replacement = generation.beginListener()
      events.add("start listener")
      events.add("start native")
      lifecycle.execute {
        assertFalse(generation.publish(replacement, 133))
        events.add("published")
      }
      true
    }, { events.add("admitted") }, { throw AssertionError(it) })
    lifecycle.next()
    lifecycle.next()
    assertFalse(generation.publish(old, 131))
    assertEquals(listOf("stop native", "start listener", "start native", "admitted", "published"), events)
  }
}
