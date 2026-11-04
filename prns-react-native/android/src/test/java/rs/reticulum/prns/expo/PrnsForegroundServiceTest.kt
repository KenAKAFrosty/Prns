package rs.reticulum.prns.expo

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import expo.modules.kotlin.Promise
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import org.mockito.Mockito.*

class PrnsForegroundServiceTest {
  abstract class TestService : PrnsForegroundService() {
    public abstract override val runtime: PrnsForegroundRuntime
  }

  private class Fixture : AutoCloseable {
    val runtime = PrnsForegroundRuntime(onUpdate = { _, _, _ -> })
    val service = mock(TestService::class.java)
    val delegate = mock(PrnsRuntimeDelegate::class.java)
    val notification = mock(Notification::class.java)
    val promise = mock(Promise::class.java)
    val intent = mock(Intent::class.java)
    val pendingIntent = mock(PendingIntent::class.java)
    private val pendingIntents = mockStatic(PendingIntent::class.java)
    private val intents = mockConstruction(Intent::class.java) { mock, _ ->
      `when`(mock.setAction(anyString())).thenReturn(mock)
    }

    init {
      `when`(service.runtime).thenReturn(runtime)
      `when`(delegate.notificationId).thenReturn(7)
      `when`(delegate.foregroundServiceType).thenReturn(16)
      `when`(delegate.notification(service, pendingIntent)).thenReturn(notification)
      pendingIntents.`when`<PendingIntent> {
        PendingIntent.getService(eq(service), eq(1), any(Intent::class.java), anyInt())
      }.thenReturn(pendingIntent)
      // Exercise the actual callback, replacing only Android framework calls.
      // Service construction/attachment is performed by Android on a device.
      field("delegate", delegate)
      doCallRealMethod().`when`(service).onStartCommand(intent, 0, 3)
    }

    fun owner() {
      assertTrue(runtime.admission.reserveOwner(service))
      field("ownsRuntime", true)
    }

    private fun field(name: String, value: Any) {
      PrnsForegroundService::class.java.getDeclaredField(name).apply {
        isAccessible = true
        set(service, value)
      }
    }

    fun start(ticket: Long) {
      runtime.requests[ticket] = promise
      `when`(intent.getLongExtra("request", 0)).thenReturn(ticket)
      assertEquals(Service.START_NOT_STICKY, service.onStartCommand(intent, 0, 3))
      val order = inOrder(service, promise)
      order.verify(service).startForeground(7, notification, 16)
      order.verify(promise).reject(eq("ERR_PRNS_CANCELLED"), anyString(), isNull())
      order.verify(service).stopSelf(3)
      assertTrue(runtime.requests.isEmpty())
      assertFalse(runtime.admission.completeStart(ticket))
      verify(delegate, never()).start(any(ByteArray::class.java) ?: byteArrayOf())
      verify(delegate, never()).savedInput()
    }

    override fun close() {
      intents.close()
      pendingIntents.close()
      runtime.lifecycle.shutdownNow()
    }
  }

  @Test fun startDuringIncompleteDrainAcknowledgesForegroundBeforeRejecting() {
    Fixture().use { fixture ->
      fixture.owner()
      val stop = fixture.runtime.admission.beginStopOperation()
      fixture.start(fixture.runtime.admission.enqueueStart())
      assertSame(fixture.service, fixture.runtime.service)
      assertTrue(fixture.runtime.admission.isStopCurrent(stop))
      assertFalse(fixture.runtime.admission.isOwnerActive(fixture.service))
    }
  }

  @Test fun refusedReplacementAcknowledgesForegroundWithoutReplacingRetainedOwner() {
    Fixture().use { fixture ->
      val retained = mock(TestService::class.java)
      assertTrue(fixture.runtime.admission.reserveOwner(retained))
      fixture.runtime.admission.beginStopOperation()
      assertFalse(fixture.runtime.admission.reserveOwner(fixture.service))
      fixture.start(fixture.runtime.admission.enqueueStart())
      assertSame(retained, fixture.runtime.service)
    }
  }

  @Test fun delayedCancelledIntentAcknowledgesForegroundWithoutStartingNativeWork() {
    Fixture().use { fixture ->
      fixture.owner()
      val ticket = fixture.runtime.admission.enqueueStart()
      fixture.runtime.admission.completeStart(ticket)
      fixture.start(ticket)
      assertTrue(fixture.runtime.admission.isOwnerActive(fixture.service))
    }
  }
}
