# PersonalRns.jl

`PersonalRns` is a thin Julia adapter over the stable native host capsule. Its concrete event, command, configuration, and outcome types are generated from the repository’s language-neutral contract. Julia multiple dispatch handles command cases directly, stream claims remain explicit values, and blocking waits can be interrupted without polling.

Install the matching native capsule and set `PRNS_HOST_LIBRARY` when it is not on the platform library path:

```julia
using PersonalRns

host = Host(ephemeral_endpoint(
    required_capabilities=Capability[PersonalRns.CapabilityTcpClient],
))

claim = claim_application_events(host)
claim isa StreamAlreadyClaimed && error("application events already have a consumer")
@async for event in claim.stream
    handle(event)
end

command = execute(
    host,
    HostCommandAttachTcpClient("127.0.0.1:4242", BitrateAuto()),
)
settlement = wait(command)
```

The package is registry-ready. Release automation can replace `PRNS_HOST_LIBRARY` with platform artifacts after the native archives receive their final immutable release URLs and hashes.
