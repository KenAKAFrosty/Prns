// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors
package rs.reticulum.prns.bluetooth

import java.util.UUID
import org.junit.Assert.*
import org.junit.Test

class ControlWriteTest {
    private val control = UUID(0, 1)
    private val data = UUID(0, 2)
    private val first = ControlWriteTicket(7, 1)
    private val second = ControlWriteTicket(7, 2)

    @Test
    fun submissionIsNotCompletionAndCallbackReturnsTheOriginalReceiptOnce() {
        val state = GattState { 0 }
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, control, first) { 0 }.admission)
        assertNull(completeGattClientWrite(state, data, 0).control)
        assertEquals(OutboundAdmission.Busy, submitGattClientWrite(state, data) { error("lane occupied") }.admission)
        assertEquals(GattWriteCompletion(true, false, first), completeGattClientWrite(state, control, 0))
        assertNull(completeGattClientWrite(state, control, 0).control)
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, data) { 0 }.admission)
        assertNull(completeGattClientWrite(state, data, 0).control)
    }

    @Test
    fun controlWaitsBehindHealthyDataWithoutFailingOrSpendingItsReceipt() {
        val state = GattState { 0 }
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, data) { 0 }.admission)
        repeat(3) {
            assertEquals(OutboundAdmission.Busy, submitGattClientWrite(state, control, first) {
                error("data still owns the physical write lane")
            }.admission)
        }
        assertEquals(GattWriteCompletion(true, false), completeGattClientWrite(state, data, 0))
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, control, first) { 0 }.admission)
        assertEquals(GattWriteCompletion(true, false, first), completeGattClientWrite(state, control, 0))
        assertTrue(state.isOpen())
    }

    @Test
    fun serverControlWaitsForDataNotificationWithoutMisattributingItsCompletion() {
        val state = GattState { 0 }
        assertTrue(state.begin(PendingGattOperation(GattOperationKind.ServerNotify, data)))
        val probe = PendingGattOperation(GattOperationKind.ServerNotify, control, first)
        assertFalse(state.begin(probe))
        assertEquals(GattWriteCompletion(true, false), completeGattServerNotify(state, 0))
        assertTrue(state.begin(probe))
        assertEquals(GattWriteCompletion(true, false, first), completeGattServerNotify(state, 0))
        assertTrue(state.isOpen())
    }

    @Test
    fun stalePumpSnapshotCannotResubmitAnAcknowledgedControl() {
        val state = GattState { 0 }
        var submitted = 0
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, control, first) { submitted++; 0 }.admission)
        val staleSnapshot = first
        assertEquals(first, completeGattClientWrite(state, control, 0).control)
        assertEquals(OutboundAdmission.Busy, submitGattClientWrite(state, control, staleSnapshot) { submitted++; 0 }.admission)
        assertEquals(1, submitted)
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, control, second) { submitted++; 0 }.admission)
        assertEquals(second, completeGattClientWrite(state, control, 0).control)
        assertEquals(OutboundAdmission.Busy, submitGattClientWrite(state, control, first) { submitted++; 0 }.admission)
        assertEquals(2, submitted)
    }

    @Test
    fun receiptExistsEvenWhenCallbackRunsBeforeThePlatformCallReturns() {
        val state = GattState { 0 }
        val receipts = mutableListOf<ControlWriteTicket>()
        val submission = submitGattClientWrite(state, control, first) {
            completeGattClientWrite(state, control, 0).control?.let(receipts::add)
            0
        }
        assertEquals(OutboundAdmission.Accepted, submission.admission)
        assertEquals(listOf(first), receipts)
        assertEquals(OutboundAdmission.Busy, submitGattClientWrite(state, control, first) { error("already sent") }.admission)
    }

    @Test
    fun busyRetriesKeepTheTicketAndDeadlineWithoutReopeningOlderReceipts() {
        var clock = 0L
        val state = GattState { clock }
        assertEquals(OutboundAdmission.Accepted, submitGattClientWrite(state, control, first) { 0 }.admission)
        assertEquals(first, completeGattClientWrite(state, control, 0).control)
        for (time in listOf(0L, 15_000L, 29_999L)) {
            clock = time
            assertEquals(OutboundAdmission.Busy, submitGattClientWrite(state, control, second) { ERROR_GATT_WRITE_REQUEST_BUSY }.admission)
            assertEquals(OutboundAdmission.Busy, submitGattClientWrite(state, control, first) { error("old ticket") }.admission)
        }
        assertTrue(state.expireOutbound(30_000, 30_000))
        assertEquals(OutboundAdmission.Busy, submitGattClientWrite(state, control, second) { error("closed") }.admission)
    }

    @Test
    fun failedClientAndServerCallbacksCaptureTheReceiptBeforeClosing() {
        val client = GattState { 0 }
        submitGattClientWrite(client, control, first) { 0 }
        assertEquals(GattWriteCompletion(true, true, first), completeGattClientWrite(client, control, 133))
        assertFalse(client.begin(PendingGattOperation(GattOperationKind.ClientWrite, control, second)))
        val server = GattState { 0 }
        assertTrue(server.begin(PendingGattOperation(GattOperationKind.ServerNotify, control, first)))
        assertEquals(GattWriteCompletion(true, true, first), completeGattServerNotify(server, 133))
        assertNull(completeGattServerNotify(server, 0).control)
    }

    @Test
    fun serverNotificationCompletionUsesItsOriginalOwnerAndRejectsStaleTickets() {
        val owners = GattServerOwners<GattState>(1)
        val state = GattState { 0 }
        assertTrue(owners.register("peer", state))
        assertTrue(state.begin(PendingGattOperation(GattOperationKind.ServerNotify, control, first)))
        assertTrue(owners.beginNotification("peer", state))
        val owner = owners.completeNotification(owners.epoch, "peer")!!
        assertEquals(GattWriteCompletion(true, false, first), completeGattServerNotify(owner, 0))
        assertFalse(state.begin(PendingGattOperation(GattOperationKind.ServerNotify, control, first)))
        assertTrue(state.begin(PendingGattOperation(GattOperationKind.ServerNotify, control, second)))
        assertTrue(owners.beginNotification("peer", state))
        assertNull(owners.completeNotification(owners.epoch - 1, "peer"))
        assertSame(state, owners.completeNotification(owners.epoch, "peer"))
        assertEquals(second, completeGattServerNotify(state, 0).control)
    }

    @Test
    fun rustCloseRequestWhileNotificationPendingKeepsTheAddressQuarantined() {
        val owners = GattServerOwners<GattState>(1)
        val state = GattState { 0 }
        assertTrue(owners.register("peer", state))
        assertTrue(state.begin(PendingGattOperation(GattOperationKind.ServerNotify, control, first)))
        assertTrue(owners.beginNotification("peer", state))
        // The exact shared helper used by both closeLink / nativeBleNextClose paths.
        val uncertain = state.closeForRetirement(false)
        assertTrue(uncertain)
        owners.retire("peer", state, disconnected = false, uncertain = uncertain)
        assertSame(state, owners.completeNotification(owners.epoch, "peer"))
        assertNull(completeGattServerNotify(state, 0).control)
        assertNull(owners.disconnected(owners.epoch, "peer"))
        assertFalse(owners.register("peer", GattState { 0 }))
    }

    @Test
    fun synchronousRejectionCanRetryButAnOldCancelCannotUndoNewOwnership() {
        val state = GattState { 0 }
        val firstOperation = PendingGattOperation(GattOperationKind.ServerNotify, control, first)
        assertTrue(state.begin(firstOperation))
        assertTrue(state.cancel(firstOperation))
        assertTrue(state.begin(firstOperation))
        assertEquals(first, completeGattServerNotify(state, 0).control)
        assertTrue(state.begin(PendingGattOperation(GattOperationKind.ServerNotify, control, second)))
        assertFalse(state.cancel(firstOperation))
        assertEquals(second, completeGattServerNotify(state, 0).control)
        assertFalse(state.begin(firstOperation.copy(control = ControlWriteTicket(8, 3))))
    }

    @Test
    fun bothConsumersCanReadTheMaximumGreetingWithoutTruncation() {
        val reader = ControlOutReader(153)
        val payload = ByteArray(153) { it.toByte() }
        val result = reader.read(42) { id, buffer, receipt ->
            assertEquals(42, id)
            assertEquals(153, buffer.capacity())
            buffer.put(payload)
            receipt[0] = first.session
            receipt[1] = first.operation
            payload.size
        } as ControlOutput.Ready
        assertArrayEquals(payload, result.payload)
        assertEquals(first, result.ticket)
    }

    @Test
    fun preAdmissionClosedIsIdleButUndersizedAndInvalidNativeBuffersAreFaults() {
        val reader = ControlOutReader(153)
        for (code in listOf(0, -1)) assertSame(ControlOutput.Idle, reader.read(1) { _, _, _ -> code })
        for (code in listOf(-2, -3, 154)) assertEquals(ControlOutput.Fault(code), reader.read(1) { _, _, _ -> code })
    }
}
