package io.reticulum.prns

import com.sun.jna.Pointer
import com.sun.jna.ptr.PointerByReference
import kotlinx.coroutines.channels.Channel
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

internal class NativeReadiness private constructor(
    private val callback: NativeReadinessCallback,
    private val registration: Pointer,
) : AutoCloseable {
    private val signal = Channel<Unit>(Channel.CONFLATED)
    private val closed = AtomicBoolean()

    suspend fun await() {
        signal.receive()
    }

    override fun close() {
        if (closed.compareAndSet(false, true)) {
            NativeApi.library.prns_readiness_registration_release(registration)
            signal.close()
        }
    }

    companion object {
        fun command(command: Pointer): NativeReadiness =
            register { callback, output ->
                NativeApi.library.prns_command_register_readiness(
                    command,
                    callback,
                    null,
                    output,
                )
            }

        fun eventStream(stream: Pointer): NativeReadiness =
            register { callback, output ->
                NativeApi.library.prns_event_stream_register_readiness(
                    stream,
                    callback,
                    null,
                    output,
                )
            }

        private fun register(
            operation: (NativeReadinessCallback, PointerByReference) -> Int,
        ): NativeReadiness {
            val target = AtomicReference<NativeReadiness>()
            val signal = NativeReadinessCallback {
                target.get()?.signal?.trySend(Unit)
            }
            val output = PointerByReference()
            checkedStatus(operation(signal, output), "registerReadiness")
            val registration = output.value
                ?: throw StatusException("registerReadiness", Status.BACKEND_FAILED)
            val readiness = NativeReadiness(signal, registration)
            target.set(readiness)
            return readiness
        }
    }
}
