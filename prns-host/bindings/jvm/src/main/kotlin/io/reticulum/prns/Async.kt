package io.reticulum.prns

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.future.future
import java.util.concurrent.CompletionStage

internal fun <Value> javaFuture(
    block: suspend CoroutineScope.() -> Value,
): CompletionStage<Value> {
    val owner = SupervisorJob()
    val future = CoroutineScope(owner + Dispatchers.Default).future(block = block)
    future.whenComplete { _, _ -> owner.cancel() }
    return future
}
