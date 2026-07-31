import asyncio
import threading

import personal_rns as prns


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


asyncio.run(main())
