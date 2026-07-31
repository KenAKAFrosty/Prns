package io.reticulum.prns

import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.filterIsInstance
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import java.io.ByteArrayOutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.nio.file.Files
import java.nio.file.Path
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
            assertEquals(BackendKind.NATIVE, host.backendInfo.backend)
            assertTrue(InterfaceKind.TCP_CLIENT in host.backendInfo.interfaceKinds)
            val initialSnapshot = host.snapshot()
            assertTrue(initialSnapshot.runtime.running)
            assertEquals(0, initialSnapshot.runtime.interfaceCount)

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
                HostCommandAttachInterface(
                    InterfaceConfigTcpClient(
                        target = "127.0.0.1:9",
                        bitrate = BitrateAuto,
                    ),
                ),
            ).use { command ->
                withTimeout(2_000) {
                    assertIs<CommandSucceeded>(command.await()).outcome
                }
            }
            val interfaceId = assertIs<CommandOutcomeInterfaceAttached>(
                attached,
            ).`interface`
            val resource = host.sendResource(
                LinkId(ByteArray(HostContract.LINK_ID_LENGTH)),
                Bytes("bounded upload".encodeToByteArray()),
                null,
                ResourceCompressionNever,
            )
            assertIs<CommandFailureUnknownLink>(
                assertIs<CommandFailed>(resource).failure,
            )
            val attachedSnapshot = host.snapshot()
            assertEquals(1, attachedSnapshot.runtime.interfaceCount)
            assertEquals(interfaceId, attachedSnapshot.interfaces.single().interfaceId)

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

    @Test
    fun persistentTwoNodeJourney(): Unit = runBlocking {
        val fixture = loadJourneyFixture()
        assertEquals(HostContract.SCHEMA_VERSION, fixture.schemaVersion)
        val port = ServerSocket(0, 50, InetAddress.getLoopbackAddress()).use {
            it.localPort
        }
        val root = Files.createTempDirectory("prns-kotlin-journey-")
        val destination = DestinationConfigSingle(
            name = DestinationName(
                appName = fixture.destination.appName,
                aspects = fixture.destination.aspects,
            ),
            identity = DestinationIdentityConfigHostIdentity,
            announceAppData = Bytes(fixture.destination.announceAppDataHex.decodeHex()),
            requestHandlers = listOf(
                RequestHandlerConfig(
                    path = fixture.request.path,
                    policy = RequestPolicy.ALLOW_ALL,
                ),
            ),
        )
        val serverOptions = HostOptions.persistentEndpoint(root.resolve("server")).copy(
            destinations = listOf(destination),
            requiredCapabilities = setOf(Capability.TCP_SERVER),
        )
        val clientOptions = HostOptions.persistentEndpoint(root.resolve("client")).copy(
            requiredCapabilities = setOf(Capability.TCP_CLIENT),
        )

        try {
            val persisted = Host(serverOptions).use { server ->
                Host(clientOptions).use { client ->
                    val destinationHash = server.destinationHashes.single()
                    val claim = assertIs<StreamClaimed<EventFlow<ApplicationEvent>>>(
                        server.claimApplicationEvents(),
                    )
                    val eventChannel = Channel<ApplicationEvent>(Channel.UNLIMITED)
                    val eventJob = launch {
                        claim.stream.collect(eventChannel::send)
                    }
                    try {
                        assertIs<CommandOutcomeInterfaceAttached>(
                            successfulOutcome(
                                settled(
                                    server,
                                    HostCommandAttachInterface(
                                        InterfaceConfigTcpServer(
                                            bind = "127.0.0.1:$port",
                                            bitrate = BitrateAuto,
                                        ),
                                    ),
                                ),
                            ),
                        )
                        assertIs<CommandOutcomeInterfaceAttached>(
                            successfulOutcome(
                                settled(
                                    client,
                                    HostCommandAttachInterface(
                                        InterfaceConfigTcpClient(
                                            target = "127.0.0.1:$port",
                                            bitrate = BitrateAuto,
                                        ),
                                    ),
                                ),
                            ),
                        )

                        var routed = false
                        repeat(50) {
                            if (!routed) {
                                routed = client.snapshot().routes.any {
                                    it.destination == destinationHash
                                }
                                if (!routed) {
                                    successfulOutcome(
                                        settled(
                                            server,
                                            HostCommandAnnounce(destinationHash, null),
                                        ),
                                    )
                                    kotlinx.coroutines.delay(50)
                                }
                            }
                        }
                        assertTrue(routed, "announced destination did not become routable")

                        val link = assertIs<CommandOutcomeLinkEstablished>(
                            successfulOutcome(
                                settled(
                                    client,
                                    HostCommandEstablishLink(destinationHash),
                                ),
                            ),
                        )
                        val requestPayload = fixture.request.payloadHex.decodeHex()
                        val responsePayload = fixture.request.responseHex.decodeHex()
                        val requestResult = async {
                            settled(
                                client,
                                HostCommandRequest(
                                    linkId = link.linkId,
                                    pathHash = RequestPathHash(
                                        fixture.request.pathHashHex.decodeHex(),
                                    ),
                                    payload = Bytes(requestPayload),
                                    timeout = ResponseTimeoutExact(
                                        fixture.request.timeoutMillis,
                                    ),
                                ),
                            )
                        }
                        val request = nextEvent<ApplicationEventRequest>(eventChannel)
                        assertContentEquals(requestPayload, request.data.copyBytes())
                        assertIs<CommandOutcomeResponseSent>(
                            successfulOutcome(
                                settled(
                                    server,
                                    HostCommandRespond(
                                        linkId = request.linkId,
                                        requestId = request.requestId,
                                        requestRttMillis = request.rttMillis,
                                        payload = Bytes(responsePayload),
                                    ),
                                ),
                            ),
                        )
                        val response = assertIs<CommandOutcomeResponseReceived>(
                            successfulOutcome(requestResult.await()),
                        )
                        assertContentEquals(responsePayload, response.data.copyBytes())

                        assertIs<CommandOutcomeResourceStrategySet>(
                            successfulOutcome(
                                settled(
                                    server,
                                    HostCommandSetLinkResourceStrategy(
                                        linkId = request.linkId,
                                        strategy = ResourceStrategyAccept(
                                            maximumUncompressedBytes = fixture.resource
                                                .maximumUncompressedBytes,
                                            acceptCompressed = fixture.resource.acceptCompressed,
                                        ),
                                    ),
                                ),
                            ),
                        )
                        val resourceChunks = fixture.resource.chunksHex.map(String::decodeHex)
                        val resourcePayload = ByteArrayOutputStream().use { output ->
                            resourceChunks.forEach(output::writeBytes)
                            output.toByteArray()
                        }
                        val metadata = fixture.resource.metadataHex.decodeHex()
                        client.beginResourceUpload(
                            linkId = link.linkId,
                            declaredLength = resourcePayload.size.toLong(),
                            packedMetadata = Bytes(metadata),
                            compression = ResourceCompressionNever,
                        ).use { upload ->
                            resourceChunks.forEach { upload.write(Bytes(it)) }
                            assertIs<CommandOutcomeResourceSent>(
                                successfulOutcome(upload.finish()),
                            )
                        }
                        val resource = nextEvent<ApplicationEventResourceAvailable>(eventChannel)
                        assertContentEquals(metadata, requireNotNull(resource.metadata).copyBytes())
                        val received = ByteArrayOutputStream()
                        resource.resource.use { stream ->
                            while (true) {
                                val chunk = stream.next(4)
                                if (chunk.finished) {
                                    break
                                }
                                received.writeBytes(chunk.bytes.copyBytes())
                            }
                        }
                        assertContentEquals(resourcePayload, received.toByteArray())
                    } finally {
                        eventJob.cancelAndJoin()
                        eventChannel.close()
                        claim.stream.close()
                    }
                    val state = Triple(
                        server.identityHash,
                        client.identityHash,
                        server.destinationHashes.single(),
                    )
                    client.stop()
                    server.stop()
                    state
                }
            }

            Host(serverOptions).use { restoredServer ->
                Host(clientOptions).use { restoredClient ->
                    assertEquals(persisted.first, restoredServer.identityHash)
                    assertEquals(persisted.second, restoredClient.identityHash)
                    assertEquals(persisted.third, restoredServer.destinationHashes.single())
                    val serverSnapshot = restoredServer.snapshot()
                    val clientSnapshot = restoredClient.snapshot()
                    assertTrue(serverSnapshot.persistence.restored)
                    assertTrue(clientSnapshot.persistence.restored)
                    assertTrue(clientSnapshot.routes.any {
                        it.destination == persisted.third
                    })
                }
            }
        } finally {
            root.toFile().deleteRecursively()
        }
    }

    private suspend fun settled(host: Host, command: HostCommand): CommandSettlement =
        host.execute(command).use {
            withTimeout(5_000) {
                it.await()
            }
        }

    private fun successfulOutcome(settlement: CommandSettlement): CommandOutcome =
        assertIs<CommandSucceeded>(settlement).outcome

    private suspend inline fun <reified Event : ApplicationEvent> nextEvent(
        channel: Channel<ApplicationEvent>,
    ): Event = withTimeout(5_000) {
        channel.receiveAsFlow().filterIsInstance<Event>().first()
    }
}

private data class JourneyFixture(
    val schemaVersion: Int,
    val destination: JourneyDestination,
    val request: JourneyRequest,
    val resource: JourneyResource,
)

private data class JourneyDestination(
    val appName: String,
    val aspects: List<String>,
    val announceAppDataHex: String,
)

private data class JourneyRequest(
    val path: String,
    val pathHashHex: String,
    val payloadHex: String,
    val responseHex: String,
    val timeoutMillis: Long,
)

private data class JourneyResource(
    val chunksHex: List<String>,
    val metadataHex: String,
    val maximumUncompressedBytes: Long,
    val acceptCompressed: Boolean,
)

private fun loadJourneyFixture(): JourneyFixture {
    val text = Files.readString(
        Path.of("..", "..", "conformance", "persistent-two-node-v2.json"),
    )
    return JourneyFixture(
        schemaVersion = text.jsonLong("schemaVersion").toInt(),
        destination = JourneyDestination(
            appName = text.jsonString("appName"),
            aspects = text.jsonStrings("aspects"),
            announceAppDataHex = text.jsonString("announceAppDataHex"),
        ),
        request = JourneyRequest(
            path = text.jsonString("path"),
            pathHashHex = text.jsonString("pathHashHex"),
            payloadHex = text.jsonString("payloadHex"),
            responseHex = text.jsonString("responseHex"),
            timeoutMillis = text.jsonLong("timeoutMillis"),
        ),
        resource = JourneyResource(
            chunksHex = text.jsonStrings("chunksHex"),
            metadataHex = text.jsonString("metadataHex"),
            maximumUncompressedBytes = text.jsonLong("maximumUncompressedBytes"),
            acceptCompressed = text.jsonBoolean("acceptCompressed"),
        ),
    )
}

private fun String.jsonString(key: String): String =
    requireNotNull(
        Regex("\\\"${Regex.escape(key)}\\\"\\s*:\\s*\\\"([^\\\"]*)\\\"").find(this),
    ).groupValues[1]

private fun String.jsonStrings(key: String): List<String> =
    requireNotNull(
        Regex("\\\"${Regex.escape(key)}\\\"\\s*:\\s*\\[([^]]*)]").find(this),
    ).groupValues[1]
        .split(',')
        .map { it.trim().removeSurrounding("\"") }

private fun String.jsonLong(key: String): Long =
    requireNotNull(
        Regex("\\\"${Regex.escape(key)}\\\"\\s*:\\s*(\\d+)").find(this),
    ).groupValues[1].toLong()

private fun String.jsonBoolean(key: String): Boolean =
    requireNotNull(
        Regex("\\\"${Regex.escape(key)}\\\"\\s*:\\s*(true|false)").find(this),
    ).groupValues[1].toBooleanStrict()

private fun String.decodeHex(): ByteArray {
    require(length % 2 == 0)
    return ByteArray(length / 2) { index ->
        substring(index * 2, index * 2 + 2).toInt(16).toByte()
    }
}
