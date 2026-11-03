// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors

package rs.reticulum.prns.bluetooth

import java.util.UUID
import org.junit.Assert.*
import org.junit.Test

class GattCapabilityNegotiationTest {
    private val capability = UUID(0, 9)
    private val control = UUID(0, 7)
    private val timeout = 8_000L
    private val bytes = byteArrayOf(42)
    private val descriptor = PendingGattOperation(GattOperationKind.DescriptorWrite, control)

    private fun state(): GattState = GattState { 0 }.also { assertTrue(it.beginStartup(0)) }

    @Test
    fun absentCharacteristicKeepsLegacyWithoutAReadOrDecoderCall() {
        val state = state()
        val negotiation = GattCapabilityNegotiation { fail("no value to decode"); true }
        assertEquals(CapabilityProgress.Ready, negotiation.start(state, null) {
            fail("absent capability must not start a platform read"); false
        })
        assertFalse(negotiation.isSupported())
        assertTrue(state.begin(descriptor))
    }

    @Test
    fun acceptedReadOwnsLaneUntilItsMatchingCallbackAndDelegatesExactValue() {
        val state = state()
        var received: ByteArray? = null
        val negotiation = GattCapabilityNegotiation { received = it; true }
        assertEquals(CapabilityProgress.Waiting, negotiation.start(state, capability) { true })
        assertFalse(negotiation.isSupported())
        assertFalse(state.markReady())
        assertFalse(state.begin(descriptor))
        assertEquals(CapabilityProgress.Ignored,
            negotiation.complete(state, control, bytes, 0, 1, timeout))
        assertFalse(state.begin(descriptor))
        assertEquals(CapabilityProgress.Ready,
            negotiation.complete(state, capability, bytes, 0, 1, timeout))
        assertSame(bytes, received)
        assertTrue(negotiation.isSupported())
        assertTrue(state.begin(descriptor))
        assertEquals(CapabilityProgress.Ignored,
            negotiation.complete(state, capability, bytes, 0, 2, timeout))
        assertFalse(state.begin(descriptor))
        assertTrue(state.complete(GattOperationKind.DescriptorWrite, control))
    }

    @Test
    fun unsupportedOrFailedCompletedReadContinuesLegacyWithoutEnforcement() {
        for (status in listOf(0, 133)) {
            val state = state()
            var decoderCalls = 0
            val negotiation = GattCapabilityNegotiation { decoderCalls++; false }
            assertEquals(CapabilityProgress.Waiting, negotiation.start(state, capability) { true })
            assertEquals(CapabilityProgress.Ready,
                negotiation.complete(state, capability, bytes, status, 1, timeout))
            assertFalse(negotiation.isSupported())
            assertEquals(if (status == 0) 1 else 0, decoderCalls)
            assertTrue(state.begin(descriptor))
        }
    }

    @Test
    fun synchronousRejectionCanContinueButOccupiedLaneCannot() {
        val state = state()
        val negotiation = GattCapabilityNegotiation { true }
        assertEquals(CapabilityProgress.Ready, negotiation.start(state, capability) { false })
        assertFalse(negotiation.isSupported())
        assertTrue(state.begin(descriptor))
        val other = GattCapabilityNegotiation { true }
        assertEquals(CapabilityProgress.Rejected, other.start(state, capability) {
            fail("another operation already owns the lane"); true
        })
        assertTrue(state.complete(GattOperationKind.DescriptorWrite, control))
    }

    @Test
    fun stalledReadUsesOriginalStartupDeadlineAndNeverPermitsLaterWrites() {
        val state = state()
        val negotiation = GattCapabilityNegotiation { true }
        // Earlier MTU/discovery work already consumed almost the whole startup budget.
        assertFalse(state.expireStartup(timeout - 1_000, timeout))
        assertEquals(CapabilityProgress.Waiting, negotiation.start(state, capability) { true })
        assertFalse(state.beginStartup(timeout - 1_000))
        assertTrue(state.expireStartup(timeout, timeout))
        assertEquals(CapabilityProgress.Ignored,
            negotiation.complete(state, capability, bytes, 0, timeout + 1, timeout))
        assertFalse(negotiation.isSupported())
        assertFalse(state.markReady())
        assertFalse(state.begin(descriptor))
    }

    @Test
    fun lateCallbackCannotBeatADelayedWatchdog() {
        val state = state()
        val negotiation = GattCapabilityNegotiation { fail("expired read"); true }
        assertEquals(CapabilityProgress.Waiting, negotiation.start(state, capability) { true })
        assertEquals(CapabilityProgress.Expired,
            negotiation.complete(state, capability, bytes, 0, timeout, timeout))
        assertFalse(negotiation.isSupported())
        assertFalse(state.begin(descriptor))
    }

    @Test
    fun immediateCallbackIsRegisteredBeforeNativeSubmissionAndCannotBeOverwritten() {
        val state = state()
        val negotiation = GattCapabilityNegotiation { true }
        assertEquals(CapabilityProgress.Waiting, negotiation.start(state, capability) {
            assertEquals(CapabilityProgress.Ready,
                negotiation.complete(state, capability, bytes, 0, 1, timeout))
            assertTrue(state.begin(descriptor))
            true
        })
        assertTrue(negotiation.isSupported())
        assertTrue(state.complete(GattOperationKind.DescriptorWrite, control))
    }

    @Test
    fun finalSubscriptionCannotAdmitLateEvenAfterATimelyCapabilityRead() {
        for (budget in listOf(timeout, 30_000L)) {
            val state = state()
            val negotiation = GattCapabilityNegotiation { true }
            assertEquals(CapabilityProgress.Waiting, negotiation.start(state, capability) { true })
            assertEquals(CapabilityProgress.Ready,
                negotiation.complete(state, capability, bytes, 0, budget - 1, budget))
            var closes = 0
            assertTrue(continueGattStartup(state, budget - 1, budget) { closes++ })
            assertTrue(state.begin(descriptor))
            // The matching callback ran before the watchdog, but after the same startup deadline.
            assertTrue(state.complete(GattOperationKind.DescriptorWrite, control))
            assertFalse(continueGattStartup(state, budget + 1, budget) { closes++ } && state.markReady())
            assertEquals(1, closes)
            assertFalse(continueGattStartup(state, budget + 2, budget) { closes++ })
            assertEquals(1, closes)
            assertFalse(state.begin(descriptor))
        }
    }

    @Test
    fun lateDiscoveryCannotStartAnOptionalReadBeforeTheWatchdogRuns() {
        for (budget in listOf(timeout, 30_000L)) {
            val state = state()
            assertTrue(state.beginServiceDiscovery())
            var closes = 0
            assertFalse(continueGattStartup(state, budget, budget) { closes++ })
            assertFalse(state.complete(GattOperationKind.ServiceDiscovery))
            val negotiation = GattCapabilityNegotiation { true }
            assertEquals(CapabilityProgress.Rejected, negotiation.start(state, capability) {
                fail("expired physical client cannot issue another read"); true
            })
            assertEquals(1, closes)
        }
    }

    @Test
    fun retiredClientCannotEnableItsReplacementAndEachAttemptStartsLegacy() {
        class Owner(val gatt: Any, val state: GattState, val negotiation: GattCapabilityNegotiation)
        val old = Owner(Any(), state(), GattCapabilityNegotiation { true })
        assertEquals(CapabilityProgress.Waiting, old.negotiation.start(old.state, capability) { true })
        old.state.close()
        val replacement = Owner(Any(), state(), GattCapabilityNegotiation { true })
        assertNull(gattCallbackOwner(replacement, old.gatt) { it.gatt })
        assertFalse(replacement.negotiation.isSupported())
        assertEquals(CapabilityProgress.Ignored,
            old.negotiation.complete(old.state, capability, bytes, 0, 1, timeout))
        assertFalse(old.negotiation.isSupported())
    }
}
