package rs.reticulum.prns

import com.sun.jna.Pointer
import com.sun.jna.ptr.ByteByReference
import com.sun.jna.ptr.PointerByReference
import java.util.concurrent.CompletionStage
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.future.await

const val SUPPLIED_PIPE_DECLINED: Int = -1

/**
 * Opens one connected POSIX descriptor when the Pipe requests a connection. A non-negative result
 * transfers ownership; a negative result or ordinary exception declines the attempt. Coroutine
 * cancellation is propagated.
 */
fun interface SuppliedPipeOpener {
    suspend fun open(): Int
}

fun interface SuppliedPipeAsyncOpener {
    fun open(): CompletionStage<Int>
}

class SuppliedPipeAttachmentException(
        val failure: CommandFailure,
) : IllegalStateException("supplied Pipe attachment failed: $failure")

/**
 * Owns one application-supplied Pipe attachment. The engine publishes open requests through this
 * controller; [serve] opens descriptors in the caller's coroutine and sends them back without
 * native code entering the JVM.
 */
class SuppliedPipe internal constructor(pointer: Pointer) : AutoCloseable {
    private val stateLock = ReentrantLock()
    private val waitLock = ReentrantLock()
    private val serving = AtomicBoolean()
    private var pointer: Pointer? = pointer
    private val attachment: Command
    private val readiness: NativeReadiness

    init {
        try {
            val output = PointerByReference()
            checkedStatus(
                    NativeApi.library.prns_supplied_pipe_claim_attachment(pointer, output),
                    "claimSuppliedPipeAttachment",
            )
            attachment = Command(requireNotNull(output.value))
            try {
                readiness = NativeReadiness.suppliedPipe(pointer)
            } catch (failure: Throwable) {
                attachment.close()
                throw failure
            }
        } catch (failure: Throwable) {
            NativeApi.library.prns_supplied_pipe_release(pointer)
            throw failure
        }
    }

    suspend fun awaitAttachment(): CommandSettlement = attachment.await()

    fun awaitAttachmentAsync(): CompletionStage<CommandSettlement> = javaFuture {
        awaitAttachment()
    }

    suspend fun serve(opener: SuppliedPipeOpener) {
        if (!serving.compareAndSet(false, true)) {
            throw StatusException("serveSuppliedPipe", Status.ALREADY_CLAIMED)
        }
        try {
            when (val settlement = awaitAttachment()) {
                is CommandSucceeded -> Unit
                is CommandFailed -> throw SuppliedPipeAttachmentException(settlement.failure)
            }
            while (true) {
                currentCoroutineContext().ensureActive()
                when (val pulled = poll()) {
                    OpenRequestPull.Empty -> if (!readiness.awaitOrClosed()) return
                    OpenRequestPull.Stopped -> return
                    is OpenRequestPull.Request ->
                            pulled.request.use { request ->
                                val descriptor =
                                        try {
                                            opener.open()
                                        } catch (failure: CancellationException) {
                                            throw failure
                                        } catch (_: Exception) {
                                            SUPPLIED_PIPE_DECLINED
                                        }
                                if (descriptor < 0) {
                                    request.decline()
                                } else {
                                    request.provide(descriptor)
                                }
                            }
                }
            }
        } finally {
            serving.set(false)
        }
    }

    fun serveAsync(opener: SuppliedPipeAsyncOpener): CompletionStage<Void?> = javaFuture {
        serve(SuppliedPipeOpener { opener.open().await() })
        null
    }

    private fun poll(): OpenRequestPull =
            waitLock.withLock {
                val native =
                        stateLock.withLock { pointer } ?: return@withLock OpenRequestPull.Stopped
                val output = PointerByReference()
                val status =
                        Status.fromRawValue(
                                NativeApi.library.prns_supplied_pipe_next_open_request(
                                        native,
                                        0,
                                        output
                                ),
                        )
                                ?: Status.BACKEND_FAILED
                when (status) {
                    Status.OK ->
                            OpenRequestPull.Request(
                                    SuppliedPipeOpenRequest(requireNotNull(output.value)),
                            )
                    Status.WOULD_BLOCK, Status.TIMED_OUT -> OpenRequestPull.Empty
                    Status.STOPPED, Status.INTERRUPTED -> OpenRequestPull.Stopped
                    else -> throw StatusException("nextSuppliedPipeOpenRequest", status)
                }
            }

    override fun close() {
        val native =
                stateLock.withLock {
                    val current = pointer
                    pointer = null
                    current?.let(NativeApi.library::prns_supplied_pipe_interrupt_wait)
                    current
                }
        if (native != null) {
            waitLock.withLock {
                readiness.close()
                NativeApi.library.prns_supplied_pipe_release(native)
            }
            attachment.close()
        }
    }
}

private sealed interface OpenRequestPull {
    data object Empty : OpenRequestPull
    data object Stopped : OpenRequestPull
    data class Request(val request: SuppliedPipeOpenRequest) : OpenRequestPull
}

private class SuppliedPipeOpenRequest(
        private var pointer: Pointer?,
) : AutoCloseable {
    fun provide(descriptor: Int): Boolean {
        val native = requireNotNull(pointer)
        val accepted = ByteByReference()
        checkedStatus(
                NativeApi.library.prns_supplied_pipe_open_request_provide(
                        native,
                        descriptor.toLong(),
                        accepted,
                ),
                "provideSuppliedPipeDescriptor",
        )
        return accepted.value.toInt() != 0
    }

    fun decline(): Boolean {
        val native = requireNotNull(pointer)
        val accepted = ByteByReference()
        checkedStatus(
                NativeApi.library.prns_supplied_pipe_open_request_decline(native, accepted),
                "declineSuppliedPipeDescriptor",
        )
        return accepted.value.toInt() != 0
    }

    override fun close() {
        pointer?.let(NativeApi.library::prns_supplied_pipe_open_request_release)
        pointer = null
    }
}
