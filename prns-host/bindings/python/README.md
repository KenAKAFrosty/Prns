# Personal RNS for Python

The Python package is a thin, typed adapter over the generated Personal RNS C host ABI. It ships the same native engine used by the .NET and C SDKs, verifies ABI and product version before node creation, and presents owned async event streams plus frozen outcome variants.

```python
from personal_rns import Host, HostOptions, IdentityConfigGenerateEphemeral

host = Host.create(
    HostOptions.endpoint(IdentityConfigGenerateEphemeral())
)

async with host:
    print(host.identity_hash)
    match host.claim_events():
        case StreamClaimed(stream):
            async for event in stream:
                match event:
                    case ApplicationEventSingleDelivery(plaintext=data):
                        print(data)
        case StreamAlreadyClaimed(lane):
            raise RuntimeError(f"{lane} already has a consumer")
```
