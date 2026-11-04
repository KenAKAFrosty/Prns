// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors

package rs.reticulum.prns.bluetooth

import java.util.UUID
import org.junit.Assert.*
import org.junit.Test

class GattLifetimeTest {
    private val characteristic = UUID(0, 1)
    private val write = PendingGattOperation(GattOperationKind.ClientWrite, characteristic)

    @Test
    fun missingClientCompletionClosesPhysicalOwnerOnceAndNeverReopensTheLane() {
        var clock = 0L
        val state = GattState { clock }
        var closes = 0
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, characteristic) { 0 }.admission)
        clock = 29_999
        assertFalse(expireGattOperation(state, clock, 30_000) { closes++ })
        clock++
        assertTrue(expireGattOperation(state, clock, 30_000) { closes++ })
        assertEquals(1, closes)
        assertFalse(state.complete(GattOperationKind.ClientWrite, characteristic))
        assertFalse(state.begin(write))
        assertFalse(expireGattOperation(state, clock + 1, 30_000) { closes++ })
        assertEquals(1, closes)
    }

    @Test
    fun completedOperationAndIdleReadyLinkDoNotExpire() {
        var clock = 0L
        val state = GattState { clock }
        assertTrue(state.markReady())
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, characteristic) { 0 }.admission)
        clock = 29_999
        assertTrue(completeGattClientWrite(state, characteristic, 0).releasedPending)
        assertFalse(expireGattOperation(state, 90_000, 30_000) { fail("completed owner must stay open") })
        clock = 90_000
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, characteristic) { 0 }.admission)
        assertFalse(state.expireOutbound(119_999, 30_000))
        assertTrue(state.expireOutbound(120_000, 30_000))
    }

    @Test
    fun laneBusyRetriesCannotRestartTheAcceptedOperationDeadline() {
        var clock = 0L
        val state = GattState { clock }
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, characteristic) { 0 }.admission)
        for (time in listOf(1L, 15_000L, 29_999L)) {
            clock = time
            assertEquals(OutboundAdmission.Busy, submitGattClientWrite(state, characteristic) {
                fail("busy lane must not submit another OS operation")
                0
            }.admission)
        }
        assertTrue(state.expireOutbound(30_000, 30_000))
    }

    @Test
    fun repeatedSynchronousPlatformBusyDoesNotRestartTheProgressDeadline() {
        var clock = 0L
        val state = GattState { clock }
        for (time in listOf(0L, 15_000L, 29_999L)) {
            clock = time
            assertEquals(OutboundAdmission.Busy, submitGattClientWrite(state, characteristic) {
                ERROR_GATT_WRITE_REQUEST_BUSY
            }.admission)
        }
        var closed = false
        assertTrue(expireGattOperation(state, 30_000, 30_000) { closed = true })
        assertTrue(closed)
        assertFalse(state.begin(write))
    }

    @Test
    fun callbackFromRetiredClientCannotResolveToReplacementPhysicalOwner() {
        class Owner(val gatt: Any)
        val previous = Owner(Any())
        val replacement = Owner(Any())
        assertSame(previous, gattCallbackOwner(previous, previous.gatt) { it.gatt })
        assertNull(gattCallbackOwner(replacement, previous.gatt) { it.gatt })
        assertSame(replacement, gattCallbackOwner(replacement, replacement.gatt) { it.gatt })
    }

    @Test
    fun serverCompletionBeforeDeadlineReleasesOnlyItsSubmittedOwner() {
        val owners = GattServerOwners<GattState>(2)
        val state = GattState { 0 }
        assertTrue(owners.register("peer", state))
        assertTrue(state.begin(PendingGattOperation(GattOperationKind.ServerNotify, characteristic)))
        assertTrue(owners.beginNotification("peer", state))
        val completed = owners.completeNotification(owners.epoch, "peer")
        assertSame(state, completed)
        assertTrue(completed!!.complete(GattOperationKind.ServerNotify))
        assertFalse(state.expireOutbound(30_000, 30_000))
    }

    @Test
    fun timedOutServerAddressStaysQuarantinedThroughLateNotificationAndDisconnect() {
        val owners = GattServerOwners<GattState>(2)
        val old = GattState { 0 }
        val replacement = GattState { 0 }
        assertTrue(owners.register("peer", old))
        assertTrue(old.begin(PendingGattOperation(GattOperationKind.ServerNotify, characteristic)))
        assertTrue(owners.beginNotification("peer", old))
        assertTrue(expireGattOperation(old, 30_000, 30_000) {
            owners.retire("peer", old, disconnected = false, uncertain = true)
        })
        assertFalse(owners.register("peer", replacement))
        assertSame(old, owners.completeNotification(owners.epoch, "peer"))
        assertFalse(old.complete(GattOperationKind.ServerNotify))
        assertNull(owners.disconnected(owners.epoch, "peer"))
        assertFalse(owners.register("peer", replacement))
        assertFalse(owners.beginNotification("peer", replacement))
    }

    @Test
    fun normalRetirementWaitsForDisconnectAndNotificationInEitherOrder() {
        for (notificationFirst in listOf(false, true)) {
            val owners = GattServerOwners<Any>(1)
            val old = Any()
            val replacement = Any()
            assertTrue(owners.register("peer", old))
            assertTrue(owners.beginNotification("peer", old))
            owners.retire("peer", old, disconnected = false, uncertain = false)
            assertFalse(owners.register("peer", replacement))
            if (notificationFirst) {
                assertSame(old, owners.completeNotification(owners.epoch, "peer"))
            } else {
                assertNull(owners.disconnected(owners.epoch, "peer"))
            }
            assertFalse(owners.register("peer", replacement))
            if (notificationFirst) {
                assertNull(owners.disconnected(owners.epoch, "peer"))
            } else {
                assertSame(old, owners.completeNotification(owners.epoch, "peer"))
            }
            assertTrue(owners.register("peer", replacement))
        }
    }

    @Test
    fun serverResetFencesOldCallbacksWithoutConsumingReplacementOwnership() {
        val owners = GattServerOwners<Any>(1)
        val old = Any()
        val replacement = Any()
        val oldEpoch = owners.epoch
        assertTrue(owners.register("peer", old))
        assertTrue(owners.beginNotification("peer", old))
        owners.retire("peer", old, disconnected = false, uncertain = true)
        owners.reset()
        assertTrue(owners.register("peer", replacement))
        assertTrue(owners.beginNotification("peer", replacement))
        assertNull(owners.completeNotification(oldEpoch, "peer"))
        assertNull(owners.disconnected(oldEpoch, "peer"))
        assertSame(replacement, owners.completeNotification(owners.epoch, "peer"))
        assertSame(replacement, owners.disconnected(owners.epoch, "peer"))
    }

    @Test
    fun tombstoneSaturationRefusesNewWorkWithoutEvictingHealthyPeers() {
        val owners = GattServerOwners<Any>(2)
        val old = Any()
        val healthy = Any()
        assertTrue(owners.register("old", old))
        assertTrue(owners.register("healthy", healthy))
        owners.retire("old", old, disconnected = false, uncertain = true)
        assertFalse(owners.canRegister("new"))
        assertFalse(owners.register("new", Any()))
        assertFalse(owners.beginNotification("new", Any()))
        assertTrue(owners.beginNotification("healthy", healthy))
        assertSame(healthy, owners.completeNotification(owners.epoch, "healthy"))
    }

    @Test
    fun synchronousNotificationRejectionReleasesOnlyTheUnstartedCallbackOwner() {
        val owners = GattServerOwners<Any>(1)
        val owner = Any()
        assertTrue(owners.register("peer", owner))
        assertTrue(owners.beginNotification("peer", owner))
        owners.cancelNotification("peer", Any())
        assertFalse(owners.beginNotification("peer", owner))
        owners.cancelNotification("peer", owner)
        assertTrue(owners.beginNotification("peer", owner))
    }
}
