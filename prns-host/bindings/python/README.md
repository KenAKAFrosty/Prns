# Personal RNS for Python

> **SDK preview: implemented, tested in-tree, and awaiting polished distribution.**
> This adapter runs the same Rust engine as every Prns node and is exercised by the repository's registered live Python conformance suite.
> Platform wheels and public-package qualification are active release work, so a source checkout is currently the supported evaluation path.
> If you are experienced with Python API or packaging design, help making this feel completely at home in Python would be especially valuable.

The Python package is a thin, typed adapter over the generated Personal RNS C host ABI. It loads the same native engine used by the .NET and C SDKs, verifies ABI and product version before node creation, and presents owned async event streams plus frozen outcome variants. Native readiness reaches `asyncio` through a nonblocking pipe on POSIX and the event-loop wake path on Windows, so command and event waits do not occupy Python worker threads.

## Evaluate the current source

On Linux, the registered suite builds the matching native capsule and runs the complete persistent two-node journey:

```console
python3 validation/run.py run --suite host-python-contract
```

The intended public delivery is a `personal-rns` platform wheel containing the matching native capsule. Until that package has completed public qualification, do not assume a package index contains the adapter from this checkout. See the [SDK guide](../../../docs/sdks.md#native-sdk-previews) for the shared release posture and contribution path.

## API shape

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
