// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors
//
// Adapted from personal-hopspot/mobile/android/app/src/main/java/org/personal/hopspot/
// BleLink.kt and NativeBridge.kt. This is Android transport glue for the app-owned
// Rust runtime, not a second Hopspot runtime. Retains the upstream dual license;
// see LICENSE-MIT and LICENSE-APACHE in the Prns repository.

package rs.reticulum.prns.app.expo

import java.nio.ByteBuffer

/** JNI transport endpoint for the existing app-owned Rust runtime. */
object PrnsBluetoothNative {
    init {
        System.loadLibrary("prns_app")
    }

    const val BLE_RADIO_ENABLED = 0x01
    const val BLE_RADIO_ADVERTISING = 0x02
    const val BLE_RADIO_SCANNING = 0x04

    external fun nativeBleSetPsm(psm: Int)

    external fun nativeBleDesiredState(): Int

    external fun nativeBlePeerCapacity(): Int

    external fun nativeBleWorkGeneration(): Long

    external fun nativeBleWaitForWork(observed: Long, timeoutMillis: Long): Long

    external fun nativeBleWakePumps()

    external fun nativeBleIdentity(buffer: ByteBuffer): Int

    external fun nativeBleSighting(address: ByteBuffer, rssi: Int)

    external fun nativeBleDialFailed(address: ByteBuffer): Boolean

    external fun nativeBleLinkUp(connId: Int, address: ByteBuffer, rssi: Int, dialed: Boolean): Boolean

    external fun nativeBleColumbaLinkUp(
        connId: Int,
        address: ByteBuffer,
        rssi: Int,
        dialed: Boolean,
        peerIdentity: ByteBuffer,
    ): Boolean

    external fun nativeBleControlIn(connId: Int, buffer: ByteBuffer, len: Int): Int

    external fun nativeBleControlOut(connId: Int, buffer: ByteBuffer): Int

    external fun nativeBleCommitControlOut(connId: Int): Boolean

    external fun nativeBleL2capIn(connId: Int, buffer: ByteBuffer, len: Int): Boolean

    external fun nativeBleL2capOut(connId: Int, buffer: ByteBuffer): Int

    external fun nativeBleDataIn(connId: Int, buffer: ByteBuffer, len: Int): Int

    external fun nativeBleDataOut(connId: Int, buffer: ByteBuffer): Int

    external fun nativeBleCommitDataOut(connId: Int): Boolean

    external fun nativeBleL2capUp(connId: Int)

    external fun nativeBleDisconnected(connId: Int)

    external fun nativeBleNextClose(): Int

    external fun nativeBleNextDial(buffer: ByteBuffer): Boolean

    external fun nativeBleNextL2capOpen(buffer: ByteBuffer): Boolean

    const val BLE_INGRESS_ACCEPTED = 0
    const val BLE_INGRESS_FULL = 1
    const val BLE_INGRESS_CLOSED = 2

}
