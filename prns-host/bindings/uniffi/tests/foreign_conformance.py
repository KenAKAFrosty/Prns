"""Run canonical two-node conformance through generated UniFFI Python bindings.

Generate bindings from the selected real native image and place that image next
to them. Pass the generated directory as the only argument to this script.
This qualifies foreign marshalling/native ownership, not Hermes/mobile runtimes.
"""
import asyncio
import json
import socket
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, sys.argv[1])
import prns_host_uniffi as p

HOST_ROOT = Path(__file__).resolve().parents[3]
SCHEMA = json.loads((HOST_ROOT / 'schema/host-contract-v1.json').read_text())
JOURNEY = json.loads((HOST_ROOT / 'conformance/persistent-two-node-v1.json').read_text())


def config(root, destinations):
    return p.HostConfig(
        identity=p.IdentityConfig.LOAD_OR_CREATE(str(root / 'identity')),
        persistence=p.PersistenceConfig.DIRECTORY(str(root)),
        role=p.HostRole.ENDPOINT, destinations=destinations, required_capabilities=[],
        limits=p.PrnsLimits(
            pending_commands=SCHEMA['limits']['pendingCommands'],
            application_events=SCHEMA['limits']['applicationEvents'],
            retained_event_bytes=SCHEMA['limits']['retainedEventBytes'],
            diagnostics=SCHEMA['limits']['diagnostics'],
        ),
    )


async def execute(host, command, expected):
    result = await host.execute(command)
    assert isinstance(result, p.CommandSettlement.SUCCEEDED), result
    assert isinstance(result.outcome, expected), result
    return result.outcome


async def next_application(stream, expected):
    async with asyncio.timeout(15):
        while await stream.ready():
            value = stream.try_next()
            if isinstance(value, p.HostEvent.APPLICATION) and isinstance(value.event, expected):
                return value
    raise AssertionError('stream ended without expected application event')


async def run():
    with tempfile.TemporaryDirectory(prefix='prns-uniffi-conformance-') as root:
        root = Path(root)
        root.joinpath('server').mkdir()
        root.joinpath('client').mkdir()
        fixture = JOURNEY['destination']
        request = JOURNEY['request']
        resource = JOURNEY['resource']
        destination = p.DestinationConfig.SINGLE(
            p.DestinationName(app_name=fixture['appName'], aspects=fixture['aspects']),
            p.DestinationIdentityConfig.HOST_IDENTITY(),
            bytes.fromhex(fixture['announceAppDataHex']), 1_048_576,
            [p.RequestHandlerConfig(path=request['path'], policy=p.RequestPolicy.ALLOW_ALL)],
        )
        server_config = config(root / 'server', [destination])
        client_config = config(root / 'client', [])
        server_owner = await p.open_host(server_config)
        client_owner = await p.open_host(client_config)
        server, client = server_owner.client(), client_owner.client()
        # Actual foreign lifting validates protocol wrappers before native admission.
        try:
            await client.remote_control_exchange(bytes(16), p.RemoteControlRequest.ACTIVATE_WIFI_CREDENTIALS(p.RemoteControlWifiCredentialRevision(value=0)))
            raise AssertionError('invalid revision accepted')
        except p.BindingError.InvalidInput:
            pass
        try:
            await client.remote_control_exchange(bytes(16), p.RemoteControlRequest.SET_INTERFACE_WIFI_STATION(
                p.RemoteControlInterfaceId(value=bytes(8)), p.RemoteControlWifiStation(ssid="", password="should-stay-secret")))
            raise AssertionError('invalid credentials accepted')
        except p.BindingError.InvalidInput as error:
            assert "should-stay-secret" not in str(error)
        failure = await client.remote_control_exchange(bytes(16), p.RemoteControlRequest.DESCRIBE())
        assert isinstance(failure, p.RemoteControlExchangeSettlement.FAILED), failure
        assert isinstance(failure.failure, p.RemoteControlNativeRemoteControlError.EXCHANGE), failure
        server_identity, client_identity = server.identity_hash(), client.identity_hash()
        destination_hash = server.destination_hashes()[0]
        events = server.application_events()
        # Cancelling a foreign readiness future must leave the queue and claim usable.
        pending = asyncio.create_task(events.ready())
        await asyncio.sleep(0)
        pending.cancel()
        try:
            await pending
            raise AssertionError('cancelled wait completed')
        except asyncio.CancelledError:
            pass
        events.close_stream()
        events = server.application_events()
        try:
            with socket.socket() as reservation:
                reservation.bind(('127.0.0.1', 0))
                port = reservation.getsockname()[1]
            await execute(server, p.HostCommand.ATTACH_TCP_SERVER(f'127.0.0.1:{port}', p.Bitrate.AUTO()), p.CommandOutcome.INTERFACE_ATTACHED)
            await execute(client, p.HostCommand.ATTACH_TCP_CLIENT(f'127.0.0.1:{port}', p.Bitrate.AUTO()), p.CommandOutcome.INTERFACE_ATTACHED)
            async with asyncio.timeout(15):
                while True:
                    await execute(server, p.HostCommand.ANNOUNCE(destination_hash, None), p.CommandOutcome.ANNOUNCED)
                    if any(route.destination == destination_hash for route in (await client.snapshot()).routes):
                        break
                    await asyncio.sleep(.05)
            link = await execute(client, p.HostCommand.ESTABLISH_LINK(destination_hash), p.CommandOutcome.LINK_ESTABLISHED)
            request_task = asyncio.create_task(execute(client, p.HostCommand.REQUEST(
                link.link_id, bytes.fromhex(request['pathHashHex']), bytes.fromhex(request['payloadHex']),
                p.ResponseTimeout.EXACT(request['timeoutMillis']), 1_048_576,
            ), p.CommandOutcome.RESPONSE_RECEIVED))
            incoming = (await next_application(events, p.ApplicationEvent.REQUEST)).event
            assert incoming.data == bytes.fromhex(request['payloadHex'])
            await execute(server, p.HostCommand.RESPOND(incoming.link_id, incoming.request_id, incoming.rtt_millis, bytes.fromhex(request['responseHex'])), p.CommandOutcome.RESPONSE_SENT)
            assert (await request_task).data == bytes.fromhex(request['responseHex'])
            await execute(server, p.HostCommand.SET_LINK_RESOURCE_STRATEGY(incoming.link_id,
                p.ResourceStrategy.ACCEPT(resource['maximumUncompressedBytes'], resource['acceptCompressed'])), p.CommandOutcome.RESOURCE_STRATEGY_SET)
            chunks = [bytes.fromhex(chunk) for chunk in resource['chunksHex']]
            upload = client.begin_resource_upload(link.link_id, sum(map(len, chunks)), bytes.fromhex(resource['metadataHex']), p.ResourceCompression.NEVER)
            receiving = asyncio.create_task(next_application(events, p.ApplicationEvent.RESOURCE_AVAILABLE))
            for chunk in chunks:
                await upload.write_chunk(chunk)
            settlement = await upload.finish()
            assert isinstance(settlement, p.CommandSettlement.SUCCEEDED), settlement
            delivered = await receiving
            assert delivered.event.metadata == bytes.fromhex(resource['metadataHex'])
            assert delivered.resource is not None
            received = bytearray()
            while (chunk := delivered.resource.read_chunk(3)) is not None:
                received.extend(chunk)
            assert received == b''.join(chunks)
            delivered.resource.close_reader()
        finally:
            events.close_stream()
            # Concurrent callers must all observe completed teardown.
            await asyncio.gather(client_owner.stop(), client_owner.stop(), server_owner.stop(), server_owner.stop())
        restored_server, restored_client = await p.open_host(server_config), await p.open_host(client_config)
        try:
            assert restored_server.client().identity_hash() == server_identity
            assert restored_client.client().identity_hash() == client_identity
            assert restored_server.client().destination_hashes()[0] == destination_hash
            assert (await restored_server.client().snapshot()).persistence.restored
            snapshot = await restored_client.client().snapshot()
            assert snapshot.persistence.restored
            assert any(route.destination == destination_hash for route in snapshot.routes)
        finally:
            await asyncio.gather(restored_server.stop(), restored_client.stop())
    print('UniFFI foreign conformance passed: cancellation/reclaim, persistent two-node request/response, streamed resource, concurrent stop, identity and route restore.')


if __name__ == '__main__':
    asyncio.run(asyncio.wait_for(run(), 60))
