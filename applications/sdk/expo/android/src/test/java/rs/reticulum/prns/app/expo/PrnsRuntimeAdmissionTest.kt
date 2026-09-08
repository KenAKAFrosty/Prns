package rs.reticulum.prns.app.expo

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class PrnsRuntimeAdmissionTest {
  private data class Owner(val name: String)

  @Test
  fun stopInvalidatesAnIntentThatHasNotArrived() {
    val admission = PrnsRuntimeAdmission<Owner>()
    val ticket = admission.enqueueStart()
    assertEquals(listOf(ticket), admission.beginStop())
    assertFalse(admission.isStartCurrent(ticket))
    assertFalse(admission.completeStart(ticket))
  }

  @Test
  fun resetInvalidatesAStartAlreadyAdmittedButStillQueued() {
    val admission = PrnsRuntimeAdmission<Owner>()
    val ticket = admission.enqueueStart()
    assertTrue(admission.isStartCurrent(ticket)) // Intent admission.
    admission.beginStop() // Reset before the queued worker runs.
    assertFalse(admission.isStartCurrent(ticket)) // Worker admission.
  }

  @Test
  fun cancellationReturnsEveryPendingPromiseIdExactlyOnce() {
    val admission = PrnsRuntimeAdmission<Owner>()
    val first = admission.enqueueStart()
    val completed = admission.enqueueStart()
    val last = admission.enqueueStart()
    assertTrue(admission.completeStart(completed))
    assertFalse(admission.completeStart(completed))
    assertEquals(listOf(first, last), admission.beginStop())
    assertEquals(emptyList<Long>(), admission.beginStop())
    assertFalse(admission.isStartCurrent(completed))
  }

  @Test
  fun aNewStartAfterCompletedStopGetsANewCurrentTicketAndOwner() {
    val admission = PrnsRuntimeAdmission<Owner>()
    val oldOwner = Owner("old")
    val newOwner = Owner("new")
    assertTrue(admission.reserveOwner(oldOwner))
    val oldTicket = admission.enqueueStart()
    admission.beginStop()
    assertTrue(admission.releaseOwner(oldOwner, drained = true))
    assertTrue(admission.reserveOwner(newOwner))
    val newTicket = admission.enqueueStart()
    assertNotEquals(oldTicket, newTicket)
    assertFalse(admission.isStartCurrent(oldTicket))
    assertTrue(admission.isStartCurrent(newTicket))
    assertSame(newOwner, admission.currentOwner())
  }

  @Test
  fun reservingTheSameOwnerIsIdempotentButAnEqualInstanceCannotReplaceIt() {
    val admission = PrnsRuntimeAdmission<Owner>()
    val owner = Owner("same-value")
    val equalButDifferentOwner = Owner("same-value")
    assertTrue(admission.reserveOwner(owner))
    assertTrue(admission.reserveOwner(owner))
    assertFalse(admission.reserveOwner(equalButDifferentOwner))
    assertSame(owner, admission.currentOwner())
  }

  @Test
  fun failedDrainRetainsTheOldOwnerAndBlocksReplacement() {
    val admission = PrnsRuntimeAdmission<Owner>()
    val oldOwner = Owner("old")
    val newOwner = Owner("new")
    assertTrue(admission.reserveOwner(oldOwner))
    assertFalse(admission.releaseOwner(oldOwner, drained = false))
    assertFalse(admission.reserveOwner(newOwner))
    assertFalse(admission.releaseOwner(newOwner, drained = true))
    assertSame(oldOwner, admission.currentOwner())
    assertTrue(admission.releaseOwner(oldOwner, drained = true))
    assertNull(admission.currentOwner())
    assertTrue(admission.reserveOwner(newOwner))
  }

  @Test
  fun aStartQueuedDuringStopCannotRestartTheRetiringOwner() {
    val admission = PrnsRuntimeAdmission<Owner>()
    val owner = Owner("stopping")
    assertTrue(admission.reserveOwner(owner))
    admission.beginStop()

    // A new Start arriving while Stop drains has a valid new ticket.
    val queued = admission.enqueueStart()
    assertTrue(admission.isStartCurrent(queued))
    // Its queued worker must still refuse this owner, before and after drain.
    assertFalse(admission.canStart(owner, queued))
    assertTrue(admission.releaseOwner(owner, drained = true))
    assertFalse(admission.canStart(owner, queued))
    assertTrue(admission.completeStart(queued))
    assertFalse(admission.completeStart(queued))
  }

  @Test
  fun failedFirstStartRetiresTheOwnerBeforeTheSecondQueuedStartRuns() {
    val admission = PrnsRuntimeAdmission<Owner>()
    val owner = Owner("starting")
    assertTrue(admission.reserveOwner(owner))
    val first = admission.enqueueStart()
    val second = admission.enqueueStart()
    assertTrue(admission.canStart(owner, first))
    assertTrue(admission.canStart(owner, second)) // Both intents already arrived.

    assertTrue(admission.retireOwner(owner)) // First Start enters failure cleanup.
    assertTrue(admission.completeStart(first))
    assertTrue(admission.isStartCurrent(second))
    assertFalse(admission.canStart(owner, second))
    assertFalse(admission.isOwnerActive(owner)) // Also fences queued radio callbacks.
    assertTrue(admission.completeStart(second))
    assertFalse(admission.completeStart(second))
  }

  @Test
  fun destructionRejectsAWorkerAdmittedBeforeTheServiceWasDestroyed() {
    val admission = PrnsRuntimeAdmission<Owner>()
    val destroyed = Owner("destroyed")
    assertTrue(admission.reserveOwner(destroyed))
    val queued = admission.enqueueStart()
    assertTrue(admission.canStart(destroyed, queued))

    assertTrue(admission.retireOwner(destroyed))
    assertFalse(admission.canStart(destroyed, queued))
    assertTrue(admission.releaseOwner(destroyed, drained = true))
    val replacement = Owner("replacement")
    assertTrue(admission.reserveOwner(replacement))
    assertFalse(admission.canStart(destroyed, queued))
    assertFalse(admission.retireOwner(destroyed))
    assertTrue(admission.isOwnerActive(replacement))
    assertTrue(admission.completeStart(queued))
  }

  @Test
  fun failedDrainAllowsOnlyTeardownUntilTheRetainedOwnerIsReleased() {
    val admission = PrnsRuntimeAdmission<Owner>()
    val retained = Owner("retained")
    val replacement = Owner("replacement")
    assertTrue(admission.reserveOwner(retained))
    admission.beginStop()
    assertFalse(admission.releaseOwner(retained, drained = false))

    val retryStart = admission.enqueueStart()
    assertTrue(admission.isStartCurrent(retryStart))
    assertFalse(admission.canStart(retained, retryStart))
    assertFalse(admission.isOwnerActive(retained))
    assertTrue(admission.reserveOwner(retained)) // Cannot reactivate the same instance.
    assertFalse(admission.isOwnerActive(retained))
    assertFalse(admission.reserveOwner(replacement))
    assertSame(retained, admission.currentOwner()) // Stop can still find the drain owner.
    assertTrue(admission.completeStart(retryStart))

    assertTrue(admission.releaseOwner(retained, drained = true)) // A later Stop succeeds.
    assertTrue(admission.reserveOwner(replacement))
    val freshStart = admission.enqueueStart()
    assertTrue(admission.canStart(replacement, freshStart))
  }
}
