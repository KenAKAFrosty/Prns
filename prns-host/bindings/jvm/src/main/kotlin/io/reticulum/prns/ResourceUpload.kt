package io.reticulum.prns

import com.sun.jna.Pointer
import com.sun.jna.ptr.PointerByReference
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.yield
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

class ResourceUpload internal constructor(pointer: Pointer) : AutoCloseable {
    private val lock = ReentrantLock()
    private var pointer: Pointer? = pointer
    private var finished = false

    suspend fun write(chunk: Bytes) {
        while (true) {
            currentCoroutineContext().ensureActive()
            val status = lock.withLock {
                val native = pointer
                    ?: throw StatusException("resourceUpload", Status.STOPPED)
                if (finished) {
                    throw StatusException("resourceUpload", Status.STOPPED)
                }
                NativeArena().use { arena ->
                    Status.fromRawValue(
                        NativeApi.library.prns_resource_upload_write(
                            native,
                            arena.bytes(chunk.copyBytes()),
                        ),
                    ) ?: Status.BACKEND_FAILED
                }
            }
            when (status) {
                Status.OK -> return
                Status.WOULD_BLOCK -> yield()
                else -> throw StatusException("writeResourceUpload", status)
            }
        }
    }

    suspend fun finish(): CommandSettlement {
        val command = lock.withLock {
            val native = pointer
                ?: throw StatusException("resourceUpload", Status.STOPPED)
            if (finished) {
                throw StatusException("resourceUpload", Status.STOPPED)
            }
            val output = PointerByReference()
            checkedStatus(
                NativeApi.library.prns_resource_upload_finish(native, output),
                "finishResourceUpload",
            )
            finished = true
            Command(requireNotNull(output.value))
        }
        return try {
            command.use { it.await() }
        } finally {
            close()
        }
    }

    fun abort() {
        lock.withLock {
            val native = pointer ?: return
            if (!finished) {
                NativeApi.library.prns_resource_upload_abort(native)
                finished = true
            }
        }
    }

    override fun close() {
        lock.withLock {
            val native = pointer ?: return
            if (!finished) {
                NativeApi.library.prns_resource_upload_abort(native)
            }
            NativeApi.library.prns_resource_upload_release(native)
            pointer = null
        }
    }
}
