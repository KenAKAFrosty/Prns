// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors

package rs.reticulum.prns.app.expo

import java.util.UUID
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PrnsGattStateTest {
    private val mtu = PendingGattOperation(GattOperationKind.Mtu, null)
    private val characteristic = UUID(0, 1)
    private val descriptor = PendingGattOperation(GattOperationKind.DescriptorWrite, characteristic)

    @Test
    fun delayedMtuKeepsDiscoveryBlockedUntilMatchingCompletion() {
        val state = PrnsGattState()
        assertTrue(state.beginStartup(0))
        assertTrue(state.begin(mtu))

        // The former fallback was 750ms; a legitimate completion may arrive later.
        assertFalse(state.expireStartup(750, 8_000))
        assertFalse(state.beginServiceDiscovery())
        assertFalse(state.begin(descriptor))

        assertFalse(state.expireStartup(900, 8_000))
        assertTrue(state.complete(GattOperationKind.Mtu))
        assertTrue(state.beginServiceDiscovery())
        assertFalse(state.complete(GattOperationKind.Mtu))
        assertFalse(state.beginServiceDiscovery())
        assertFalse(state.begin(descriptor))
        assertTrue(state.complete(GattOperationKind.ServiceDiscovery))
        assertFalse(state.beginServiceDiscovery())
        assertTrue(state.begin(descriptor))
    }

    @Test
    fun rejectedMtuRequestAllowsDiscoveryWithoutWaitingForCallback() {
        val state = PrnsGattState()
        assertTrue(state.beginStartup(0))
        assertTrue(state.begin(mtu))
        assertTrue(state.cancel(mtu))
        assertTrue(state.beginServiceDiscovery())
        assertFalse(state.cancel(mtu))
        assertFalse(state.complete(GattOperationKind.Mtu))
        assertFalse(state.beginServiceDiscovery())
        assertTrue(state.complete(GattOperationKind.ServiceDiscovery))
    }

    @Test
    fun missingMtuCallbackClosesAtTheTerminalDeadlineWithoutDiscovery() {
        val state = PrnsGattState()
        assertTrue(state.beginStartup(0))
        assertTrue(state.begin(mtu))
        assertFalse(state.expireStartup(750, 8_000))
        assertFalse(state.expireStartup(7_999, 8_000))
        assertFalse(state.beginServiceDiscovery())
        assertTrue(state.expireStartup(8_000, 8_000))
        assertFalse(state.expireStartup(8_001, 8_000))
        assertFalse(state.complete(GattOperationKind.Mtu))
        assertFalse(state.beginServiceDiscovery())
        assertFalse(state.begin(descriptor))
        assertFalse(state.markReady())
    }

    @Test
    fun duplicateConnectionDoesNotRestartStartupOrReleasePendingMtu() {
        val state = PrnsGattState()
        assertTrue(state.beginStartup(0))
        assertTrue(state.begin(mtu))
        assertFalse(state.beginStartup(500))
        assertFalse(state.begin(mtu))
        assertFalse(state.cancel(descriptor))
        assertFalse(state.complete(GattOperationKind.DescriptorWrite, characteristic))
        assertFalse(state.beginServiceDiscovery())
        assertTrue(state.expireStartup(8_000, 8_000))
    }

    @Test
    fun teardownFencesLateCallbacksAndFurtherOperations() {
        val state = PrnsGattState()
        assertTrue(state.beginStartup(0))
        assertTrue(state.begin(mtu))
        state.close()
        state.close()
        assertFalse(state.cancel(mtu))
        assertFalse(state.complete(GattOperationKind.Mtu))
        assertFalse(state.beginServiceDiscovery())
        assertFalse(state.begin(mtu))
        assertFalse(state.beginStartup(900))
        assertFalse(state.markReady())
        assertFalse(state.expireStartup(8_000, 8_000))
    }

    @Test
    fun readyLinkIsNotExpiredByTheStartupWatchdog() {
        val state = PrnsGattState()
        assertTrue(state.beginStartup(0))
        assertTrue(state.begin(mtu))
        assertTrue(state.complete(GattOperationKind.Mtu))
        assertTrue(state.beginServiceDiscovery())
        assertTrue(state.complete(GattOperationKind.ServiceDiscovery))
        assertTrue(state.begin(descriptor))
        assertTrue(state.complete(GattOperationKind.DescriptorWrite, characteristic))
        assertTrue(state.markReady())
        assertFalse(state.markReady())
        assertTrue(state.begin(PendingGattOperation(GattOperationKind.ClientWrite, characteristic)))
        assertFalse(state.expireStartup(8_000, 8_000))
        assertTrue(state.complete(GattOperationKind.ClientWrite, characteristic))
    }

    @Test
    fun duplicateIdentityReadCannotReleaseTheFollowingDescriptorWrite() {
        val state = PrnsGattState()
        assertTrue(state.begin(PendingGattOperation(GattOperationKind.CharacteristicRead, characteristic)))
        assertTrue(state.complete(GattOperationKind.CharacteristicRead, characteristic))
        assertTrue(state.begin(descriptor))
        assertFalse(state.complete(GattOperationKind.CharacteristicRead, characteristic))
        assertFalse(state.begin(descriptor))
        assertTrue(state.complete(GattOperationKind.DescriptorWrite, characteristic))
        state.close()
        assertFalse(state.complete(GattOperationKind.CharacteristicRead, characteristic))
    }

    @Test
    fun serverNotificationCompletionStillAllowsAWildcardCharacteristic() {
        val state = PrnsGattState()
        val notification = PendingGattOperation(GattOperationKind.ServerNotify, characteristic)
        assertTrue(state.begin(notification))
        assertFalse(state.complete(GattOperationKind.ServerNotify, UUID(0, 2)))
        assertFalse(state.begin(notification))
        assertTrue(state.complete(GattOperationKind.ServerNotify))
        assertFalse(state.complete(GattOperationKind.ServerNotify))
        assertTrue(state.begin(notification))
    }
}
