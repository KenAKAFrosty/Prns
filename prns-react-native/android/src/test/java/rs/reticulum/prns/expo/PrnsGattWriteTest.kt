// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors

package rs.reticulum.prns.expo

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
        val state = PrnsGattState()
        val queued = ArrayDeque(listOf(88, 180, 47))
        val submitted = mutableListOf<Int>()
        var platformBusy = false
        fun pump(): OutboundAdmission {
            val next = queued.first()
            val result = prnsSubmitGattClientWrite(state, data) {
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
                prnsCompleteGattClientWrite(state, data, BluetoothGatt.GATT_SUCCESS),
            )
        }
        assertEquals(listOf(88, 180, 47), submitted)
        assertTrue(queued.isEmpty())
    }

    @Test
    fun controlAndDataShareTheSameLaneAndMismatchedSuccessCannotReleaseIt() {
        val state = PrnsGattState()
        assertEquals(OutboundAdmission.Accepted, prnsSubmitGattClientWrite(state, data) { 0 }.admission)
        assertEquals(
            GattWriteCompletion(releasedPending = false, shouldClose = false),
            prnsCompleteGattClientWrite(state, control, 0),
        )
        assertEquals(OutboundAdmission.Busy, prnsSubmitGattClientWrite(state, control) {
            throw AssertionError("control must not overlap an admitted data write")
        }.admission)
        assertTrue(prnsCompleteGattClientWrite(state, data, 0).releasedPending)
        assertEquals(OutboundAdmission.Accepted, prnsSubmitGattClientWrite(state, control) { 0 }.admission)
    }

    @Test
    fun platformBusyReleasesOnlyTheUnstartedOperationForExplicitPumpRetry() {
        val state = PrnsGattState()
        assertEquals(
            GattWriteSubmission(OutboundAdmission.Busy, ERROR_GATT_WRITE_REQUEST_BUSY),
            prnsSubmitGattClientWrite(state, data) { ERROR_GATT_WRITE_REQUEST_BUSY },
        )
        assertEquals(OutboundAdmission.Accepted, prnsSubmitGattClientWrite(state, data) { 0 }.admission)
    }

    @Test
    fun legacyFalseRemainsTerminalInsteadOfGuessingItWasBusy() {
        val state = PrnsGattState()
        assertEquals(
            GattWriteSubmission(OutboundAdmission.Terminal, BluetoothGatt.GATT_FAILURE),
            prnsSubmitGattClientWrite(state, data) { BluetoothGatt.GATT_FAILURE },
        )
        assertFalse(state.complete(GattOperationKind.ClientWrite, data))
    }

    @Test
    fun failedCallbackClosesEvenWithoutAMatchingPendingWrite() {
        val state = PrnsGattState()
        assertEquals(
            GattWriteCompletion(releasedPending = false, shouldClose = true),
            prnsCompleteGattClientWrite(state, data, 133),
        )
        assertFalse(state.begin(PendingGattOperation(GattOperationKind.ClientWrite, data)))
    }

    @Test
    fun failedMatchingCallbackReleasesAndClosesBeforeAnotherWriteCanStart() {
        val state = PrnsGattState()
        assertEquals(OutboundAdmission.Accepted, prnsSubmitGattClientWrite(state, data) { 0 }.admission)
        assertEquals(
            GattWriteCompletion(releasedPending = true, shouldClose = true),
            prnsCompleteGattClientWrite(state, data, 17),
        )
        assertFalse(state.begin(PendingGattOperation(GattOperationKind.ClientWrite, control)))
    }

    @Test
    fun throwingPlatformRequestDoesNotLeaveAnUnstartedOperationInFlight() {
        val state = PrnsGattState()
        try {
            prnsSubmitGattClientWrite(state, data) { throw SecurityException("denied") }
            throw AssertionError("expected platform exception")
        } catch (_: SecurityException) {
            assertEquals(OutboundAdmission.Accepted, prnsSubmitGattClientWrite(state, data) { 0 }.admission)
        }
    }
}
