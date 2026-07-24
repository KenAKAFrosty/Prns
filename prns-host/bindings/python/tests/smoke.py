import asyncio

import personal_rns as prns


async def main():
    host = prns.Host.create(
        prns.HostOptions.endpoint(prns.IdentityConfigGenerateEphemeral())
    )
    assert host.lifecycle.phase is prns.LifecyclePhase.RUNNING
    assert len(host.identity_hash.value) == prns.IDENTITY_HASH_LENGTH
    first = host.claim_events()
    assert isinstance(first, prns.StreamClaimed)
    assert isinstance(host.claim_events(), prns.StreamAlreadyClaimed)
    pending = asyncio.create_task(first.stream.__anext__())
    await asyncio.sleep(0)
    pending.cancel()
    try:
        await pending
    except asyncio.CancelledError:
        pass
    await first.stream.aclose()
    settled = await host.close_link(prns.LinkId(bytes(prns.LINK_ID_LENGTH)))
    assert isinstance(settled, prns.CommandSucceeded)
    assert isinstance(settled.outcome, prns.CommandOutcomeLinkCloseQueued)
    await host.aclose()


asyncio.run(main())
