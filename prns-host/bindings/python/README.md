# Personal RNS for Python

> **Status: solid core, young surface.**
> This binding runs the same Rust engine as every Prns node and passes the same cross-language conformance suite on every release.
> The young part is the Python-facing API. Its shape is a working first draft: a starting point, not the final word.
> If you are an experienced Python developer and something here does not feel native, that is exactly the feedback we want. Issues and PRs on API design are among the most valuable contributions right now.

The Python package is a thin, typed adapter over the generated Personal RNS C host ABI. It ships the same native engine used by the .NET and C SDKs, verifies ABI and product version before node creation, and presents owned async event streams plus frozen outcome variants. Native readiness reaches `asyncio` through a nonblocking pipe on POSIX and the event-loop wake path on Windows, so command and event waits do not occupy Python worker threads.

```python
from personal_rns import (
    ApplicationEventSingleDelivery,
    Host,
    HostOptions,
    IdentityConfigGenerateEphemeral,
    StreamAlreadyClaimed,
)

host = Host.create(
    HostOptions.endpoint(IdentityConfigGenerateEphemeral())
)

async with host:
    print(host.identity_hash)
    claim = host.claim_events()
    if isinstance(claim, StreamAlreadyClaimed):
        raise RuntimeError(f"{claim.lane} already has a consumer")
    async for event in claim.stream:
        match event:
            case ApplicationEventSingleDelivery(plaintext=data):
                print(data)
```
