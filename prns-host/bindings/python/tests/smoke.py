import asyncio
import json
import socket
import tempfile
import threading
from pathlib import Path

import personal_rns as prns


JOURNEY = json.loads(
    (Path(__file__).parents[3] / "conformance/persistent-two-node-v2.json")
    .read_text()
)


async def next_matching(stream, event_type, timeout=5):
    async def read():
        async for event in stream:
            if isinstance(event, event_type):
                return event
        raise AssertionError("event stream stopped before the expected event")

    return await asyncio.wait_for(read(), timeout)


async def wait_for_route(host, destination, announcer):
    for _ in range(50):
        if any(route.destination == destination for route in host.snapshot().routes):
            return
        await announcer()
        await asyncio.sleep(0.05)
    raise AssertionError("announced destination did not become routable")


async def persistent_two_node_journey():
    with tempfile.TemporaryDirectory(prefix="prns-python-journey-") as root:
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        destination_fixture = JOURNEY["destination"]
        request_fixture = JOURNEY["request"]
        resource_fixture = JOURNEY["resource"]
        request_payload = bytes.fromhex(request_fixture["payloadHex"])
        response_payload = bytes.fromhex(request_fixture["responseHex"])
        resource_chunks = [
            bytes.fromhex(chunk) for chunk in resource_fixture["chunksHex"]
        ]
        resource_payload = b"".join(resource_chunks)
        resource_metadata = bytes.fromhex(resource_fixture["metadataHex"])
        destination = prns.DestinationConfigSingle(
            prns.DestinationName(
                destination_fixture["appName"],
                tuple(destination_fixture["aspects"]),
            ),
            prns.DestinationIdentityConfigHostIdentity(),
            bytes.fromhex(destination_fixture["announceAppDataHex"]),
            (
                prns.RequestHandlerConfig(
                    request_fixture["path"],
                    prns.RequestPolicy.ALLOW_ALL,
                ),
            ),
        )
        server_options = prns.HostOptions.persistent_endpoint(
            f"{root}/server",
            (destination,),
            (prns.Capability.TCP_SERVER,),
        )
        client_options = prns.HostOptions.persistent_endpoint(
            f"{root}/client",
            (),
            (prns.Capability.TCP_CLIENT,),
        )
        server = prns.Host.create(server_options)
        client = prns.Host.create(client_options)
        server_identity = server.identity_hash
        client_identity = client.identity_hash
        destination_hash = server.destination_hashes[0]
        server_claim = server.claim_events()
        assert isinstance(server_claim, prns.StreamClaimed)
        server_events = server_claim.stream
        try:
            listening = await server.attach_interface(
                prns.InterfaceConfigTcpServer(
                    f"127.0.0.1:{port}",
                    prns.BitrateAuto(),
                )
            )
            assert isinstance(listening, prns.CommandSucceeded)
            assert isinstance(
                listening.outcome,
                prns.CommandOutcomeInterfaceAttached,
            )
            connected = await client.attach_interface(
                prns.InterfaceConfigTcpClient(
                    f"127.0.0.1:{port}",
                    prns.BitrateAuto(),
                )
            )
            assert isinstance(connected, prns.CommandSucceeded)
            assert isinstance(
                connected.outcome,
                prns.CommandOutcomeInterfaceAttached,
            )
            await wait_for_route(
                client,
                destination_hash,
                lambda: server.announce(destination_hash),
            )
            link = await client.establish_link(destination_hash)
            assert isinstance(link, prns.CommandSucceeded)
            assert isinstance(link.outcome, prns.CommandOutcomeLinkEstablished)
            request_task = asyncio.create_task(
                client.request(
                    link.outcome.link_id,
                    prns.RequestPathHash(
                        bytes.fromhex(request_fixture["pathHashHex"])
                    ),
                    request_payload,
                    prns.ResponseTimeoutExact(request_fixture["timeoutMillis"]),
                )
            )
            request = await next_matching(
                server_events,
                prns.ApplicationEventRequest,
            )
            assert request.data == request_payload
            response = await server.respond(
                request.link_id,
                request.request_id,
                request.rtt_millis,
                response_payload,
            )
            assert isinstance(response, prns.CommandSucceeded)
            request_settlement = await request_task
            assert isinstance(request_settlement, prns.CommandSucceeded)
            assert isinstance(
                request_settlement.outcome,
                prns.CommandOutcomeResponseReceived,
            )
            assert request_settlement.outcome.data == response_payload
            strategy = await server.set_link_resource_strategy(
                request.link_id,
                prns.ResourceStrategyAccept(
                    resource_fixture["maximumUncompressedBytes"],
                    resource_fixture["acceptCompressed"],
                ),
            )
            assert isinstance(strategy, prns.CommandSucceeded)

            async def chunks():
                for chunk in resource_chunks:
                    yield chunk

            resource_event_task = asyncio.create_task(
                next_matching(
                    server_events,
                    prns.ApplicationEventResourceAvailable,
                )
            )
            resource_sent = await client.send_resource_stream(
                link.outcome.link_id,
                len(resource_payload),
                chunks(),
                resource_metadata,
                prns.ResourceCompressionNever(),
            )
            assert isinstance(resource_sent, prns.CommandSucceeded)
            resource_event = await resource_event_task
            assert resource_event.metadata == resource_metadata
            received = bytearray()
            async for chunk in resource_event.resource:
                received.extend(chunk)
            assert received == resource_payload
            assert any(
                route.destination == destination_hash
                for route in client.snapshot().routes
            )
        finally:
            await server_events.aclose()
            await client.aclose()
            await server.aclose()

        restored_server = prns.Host.create(server_options)
        restored_client = prns.Host.create(client_options)
        try:
            assert restored_server.identity_hash == server_identity
            assert restored_client.identity_hash == client_identity
            assert restored_server.destination_hashes[0] == destination_hash
            assert restored_server.snapshot().persistence.restored
            restored_client_snapshot = restored_client.snapshot()
            assert restored_client_snapshot.persistence.restored
            assert any(
                route.destination == destination_hash
                for route in restored_client_snapshot.routes
            )
        finally:
            await restored_client.aclose()
            await restored_server.aclose()


async def main():
    host = prns.Host.create(
        prns.HostOptions.endpoint(prns.IdentityConfigGenerateEphemeral())
    )
    assert host.lifecycle.phase is prns.LifecyclePhase.RUNNING
    assert host.backend_info.backend is prns.BackendKind.NATIVE
    assert prns.InterfaceKind.TCP_CLIENT in host.backend_info.interface_kinds
    initial_snapshot = host.snapshot()
    assert initial_snapshot.runtime.running
    assert initial_snapshot.runtime.interface_count == 0
    assert len(host.identity_hash.value) == prns.IDENTITY_HASH_LENGTH
    first = host.claim_events()
    assert isinstance(first, prns.StreamClaimed)
    assert isinstance(host.claim_events(), prns.StreamAlreadyClaimed)
    python_threads = threading.active_count()
    pending = asyncio.create_task(first.stream.__anext__())
    await asyncio.sleep(0)
    assert threading.active_count() == python_threads
    pending.cancel()
    try:
        await pending
    except asyncio.CancelledError:
        pass
    await first.stream.aclose()
    second = host.claim_events()
    assert isinstance(second, prns.StreamClaimed)
    pending = asyncio.create_task(second.stream.__anext__())
    await asyncio.sleep(0)
    await second.stream.aclose()
    try:
        await pending
        raise AssertionError("closed event stream produced an event")
    except StopAsyncIteration:
        pass
    settled = await host.close_link(prns.LinkId(bytes(prns.LINK_ID_LENGTH)))
    assert isinstance(settled, prns.CommandSucceeded)
    assert isinstance(settled.outcome, prns.CommandOutcomeLinkCloseQueued)
    resource = await host.send_resource(
        prns.LinkId(bytes(prns.LINK_ID_LENGTH)),
        b"bounded upload",
        None,
        prns.ResourceCompressionNever(),
    )
    assert isinstance(resource, prns.CommandFailed)
    assert isinstance(resource.failure, prns.CommandFailureUnknownLink)
    attached = await host.attach_interface(
        prns.InterfaceConfigTcpClient("127.0.0.1:9", prns.BitrateAuto())
    )
    assert isinstance(attached, prns.CommandSucceeded)
    assert isinstance(attached.outcome, prns.CommandOutcomeInterfaceAttached)
    attached_snapshot = host.snapshot()
    assert attached_snapshot.runtime.interface_count == 1
    assert attached_snapshot.interfaces[0].interface_id == attached.outcome.interface
    detached = await host.detach_interface(attached.outcome.interface)
    assert isinstance(detached, prns.CommandSucceeded)
    assert isinstance(detached.outcome, prns.CommandOutcomeInterfaceDetached)
    await host.aclose()
    await persistent_two_node_journey()


asyncio.run(main())
