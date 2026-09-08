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
    assertEquals(listOf(ticket), admission.invalidateStarts())
    assertFalse(admission.isStartCurrent(ticket))
    assertFalse(admission.completeStart(ticket))
  }

  @Test
  fun resetInvalidatesAStartAlreadyAdmittedButStillQueued() {
    val admission = PrnsRuntimeAdmission<Owner>()
    val ticket = admission.enqueueStart()
    assertTrue(admission.isStartCurrent(ticket)) // Intent admission.
    admission.invalidateStarts() // Reset before the queued worker runs.
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
    assertEquals(listOf(first, last), admission.invalidateStarts())
    assertEquals(emptyList<Long>(), admission.invalidateStarts())
    assertFalse(admission.isStartCurrent(completed))
  }

  @Test
  fun aNewStartAfterCompletedStopGetsANewCurrentTicketAndOwner() {
    val admission = PrnsRuntimeAdmission<Owner>()
    val oldOwner = Owner("old")
    val newOwner = Owner("new")
    assertTrue(admission.reserveOwner(oldOwner))
    val oldTicket = admission.enqueueStart()
    admission.invalidateStarts()
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
}
