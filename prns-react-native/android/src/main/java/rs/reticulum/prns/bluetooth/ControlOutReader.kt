// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors
package rs.reticulum.prns.bluetooth

import java.nio.ByteBuffer

internal sealed class ControlOutput {
    object Idle : ControlOutput()
    data class Fault(val code: Int) : ControlOutput()
    data class Ready(val payload: ByteArray, val ticket: ControlWriteTicket) : ControlOutput()
}

/** Both Android consumers use the native codec's capacity and receipt contract. */
internal class ControlOutReader(capacity: Int) {
    private val buffer = ByteBuffer.allocateDirect(capacity.also { require(it > 0) })
    private val receipt = LongArray(2)

    fun read(connId: Int, receive: (Int, ByteBuffer, LongArray) -> Int): ControlOutput {
        buffer.clear()
        return when (val length = receive(connId, buffer, receipt)) {
            // A physical link exists before Rust admission during GATT startup.
            // Rust retirement is separately acknowledged by the close pump.
            0, -1 -> ControlOutput.Idle
            in 1..buffer.capacity() -> {
                buffer.position(0)
                val payload = ByteArray(length)
                buffer.get(payload)
                ControlOutput.Ready(payload, ControlWriteTicket(receipt[0], receipt[1]))
            }
            else -> ControlOutput.Fault(length)
        }
    }
}
