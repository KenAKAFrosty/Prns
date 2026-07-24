package io.reticulum.prns

import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue

class HostContractTest {
    @Test
    fun bytesAreImmutableValues() {
        val source = byteArrayOf(1, 2, 3)
        val value = Bytes(source)
        source[0] = 9
        assertContentEquals(byteArrayOf(1, 2, 3), value.copyBytes())
        val copy = value.copyBytes()
        copy[1] = 9
        assertContentEquals(byteArrayOf(1, 2, 3), value.copyBytes())
        assertEquals(Bytes(byteArrayOf(1, 2, 3)), value)
    }

    @Test
    fun nativeHostContract(): Unit = runBlocking {
        Host(
            HostOptions(
                role = HostRole.ENDPOINT,
                identity = IdentityConfigGenerateEphemeral,
                destinations = emptyList(),
                requiredCapabilities = setOf(Capability.TCP_CLIENT),
            ),
        ).use { host ->
            assertNotEquals(
                IdentityHash(ByteArray(HostContract.IDENTITY_HASH_LENGTH)),
                host.identityHash,
            )

            val firstClaim = assertIs<StreamClaimed<EventFlow<ApplicationEvent>>>(
                host.claimApplicationEvents(),
            )
            assertIs<StreamAlreadyClaimed>(host.claimApplicationEvents())
            try {
                withTimeout(20) {
                    firstClaim.stream.first()
                }
                error("cancelled event wait completed successfully")
            } catch (_: TimeoutCancellationException) {
            }

            val attached = host.execute(
                HostCommandAttachTcpClient(
                    target = "127.0.0.1:9",
                    bitrate = BitrateAuto,
                ),
            ).use { command ->
                withTimeout(2_000) {
                    assertIs<CommandSucceeded>(command.await()).outcome
                }
            }
            val interfaceId = assertIs<CommandOutcomeInterfaceAttached>(
                attached,
            ).`interface`

            val detached = host.execute(
                HostCommandDetachInterface(interfaceId),
            ).use { command ->
                withTimeout(2_000) {
                    assertIs<CommandSucceeded>(command.await()).outcome
                }
            }
            assertContentEquals(
                interfaceId.copyBytes(),
                assertIs<CommandOutcomeInterfaceDetached>(detached)
                    .`interface`
                    .copyBytes(),
            )
            assertTrue(host.destinationHashes.isEmpty())
        }
    }
}
