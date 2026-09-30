// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors

package rs.reticulum.prns.expo

import rs.reticulum.prns.bluetooth.ERROR_GATT_WRITE_REQUEST_BUSY
import rs.reticulum.prns.bluetooth.GattOperationKind
import rs.reticulum.prns.bluetooth.GattState
import rs.reticulum.prns.bluetooth.GattWriteCompletion
import rs.reticulum.prns.bluetooth.GattWriteSubmission
import rs.reticulum.prns.bluetooth.OutboundAdmission
import rs.reticulum.prns.bluetooth.PendingGattOperation
import rs.reticulum.prns.bluetooth.completeGattClientWrite
import rs.reticulum.prns.bluetooth.submitGattClientWrite

import android.bluetooth.BluetoothGatt
import java.util.UUID
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PrnsGattWriteTest {
    private val data = UUID(0, 1)
    private val control = UUID(0, 2)

    @Test
    fun noResponseFragmentsRetainTheirQueueHeadUntilAndroidCompletesThePreviousWrite() {
        val state = GattState()
        val queued = ArrayDeque(listOf(88, 180, 47))
        val submitted = mutableListOf<Int>()
        var platformBusy = false
        fun pump(): OutboundAdmission {
            val next = queued.first()
            val result = submitGattClientWrite(state, data) {
                // The legacy API reports false (mapped to 257) for overlapping
                // writes, even when they do not request an ATT response.
                if (platformBusy) BluetoothGatt.GATT_FAILURE else {
                    platformBusy = true
                    submitted.add(next)
                    BluetoothGatt.GATT_SUCCESS
                }
            }
            if (result.admission == OutboundAdmission.Accepted) queued.removeFirst()
            return result.admission
        }
        repeat(3) { index ->
            assertEquals(OutboundAdmission.Accepted, pump())
            if (queued.isNotEmpty()) {
                repeat(2) { assertEquals(OutboundAdmission.Busy, pump()) }
                assertEquals(index + 1, submitted.size)
            }
            platformBusy = false
            assertEquals(
                GattWriteCompletion(releasedPending = true, shouldClose = false),
                completeGattClientWrite(state, data, BluetoothGatt.GATT_SUCCESS),
            )
        }
        assertEquals(listOf(88, 180, 47), submitted)
        assertTrue(queued.isEmpty())
    }

    @Test
    fun controlAndDataShareTheSameLaneAndMismatchedSuccessCannotReleaseIt() {
        val state = GattState()
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, data) { 0 }.admission)
        assertEquals(
            GattWriteCompletion(releasedPending = false, shouldClose = false),
            completeGattClientWrite(state, control, 0),
        )
        assertEquals(OutboundAdmission.Busy, submitGattClientWrite(state, control) {
            throw AssertionError("control must not overlap an admitted data write")
        }.admission)
        assertTrue(completeGattClientWrite(state, data, 0).releasedPending)
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, control) { 0 }.admission)
    }

    @Test
    fun platformBusyReleasesOnlyTheUnstartedOperationForExplicitPumpRetry() {
        val state = GattState()
        assertEquals(
            GattWriteSubmission(OutboundAdmission.Busy, ERROR_GATT_WRITE_REQUEST_BUSY),
            submitGattClientWrite(state, data) { ERROR_GATT_WRITE_REQUEST_BUSY },
        )
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, data) { 0 }.admission)
    }

    @Test
    fun legacyFalseRemainsTerminalInsteadOfGuessingItWasBusy() {
        val state = GattState()
        assertEquals(
            GattWriteSubmission(OutboundAdmission.Terminal, BluetoothGatt.GATT_FAILURE),
            submitGattClientWrite(state, data) { BluetoothGatt.GATT_FAILURE },
        )
        assertFalse(state.complete(GattOperationKind.ClientWrite, data))
    }

    @Test
    fun failedCallbackClosesEvenWithoutAMatchingPendingWrite() {
        val state = GattState()
        assertEquals(
            GattWriteCompletion(releasedPending = false, shouldClose = true),
            completeGattClientWrite(state, data, 133),
        )
        assertFalse(state.begin(PendingGattOperation(GattOperationKind.ClientWrite, data)))
    }

    @Test
    fun failedMatchingCallbackReleasesAndClosesBeforeAnotherWriteCanStart() {
        val state = GattState()
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, data) { 0 }.admission)
        assertEquals(
            GattWriteCompletion(releasedPending = true, shouldClose = true),
            completeGattClientWrite(state, data, 17),
        )
        assertFalse(state.begin(PendingGattOperation(GattOperationKind.ClientWrite, control)))
    }

    @Test
    fun throwingPlatformRequestDoesNotLeaveAnUnstartedOperationInFlight() {
        val state = GattState()
        try {
            submitGattClientWrite(state, data) { throw SecurityException("denied") }
            throw AssertionError("expected platform exception")
        } catch (_: SecurityException) {
            assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, data) { 0 }.admission)
        }
    }
}
