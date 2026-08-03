# PersonalRns.jl

> **SDK preview: implemented, tested in-tree, and awaiting polished distribution.**
> This adapter runs the same Rust engine as every Prns node and is exercised by the repository's registered live Julia conformance suite.
> Julia General registration, matching native artifacts, and public-package qualification are active release work, so a source checkout is currently the supported evaluation path.
> If you are experienced with Julia API, package, or artifact design, help making this feel completely at home in Julia would be especially valuable.

`PersonalRns` is a thin Julia adapter over the stable native host capsule. Its concrete event, command, configuration, and outcome types are generated from the repository’s language-neutral contract. Julia multiple dispatch handles command cases directly, stream claims remain explicit values, and native readiness wakes Julia tasks through libuv without polling or occupying a worker thread.

## Evaluate the current source

On Linux, the registered suite builds the matching native capsule and runs the
complete persistent two-node journey with both single-threaded and
multi-threaded Julia:

```console
python3 validation/run.py run --suite host-julia-contract
```

The intended public delivery is a Julia General package whose generated
artifact metadata resolves the matching native capsule. Until registration and
public qualification are complete, do not assume the registry contains this
checkout. Source-tree development sets `PRNS_HOST_LIBRARY` to an explicit
native capsule. See the [SDK guide](../../../docs/sdks.md#native-sdk-previews)
for the shared release posture and contribution path.

## API shape

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

settlement = attach_tcp_client(
    host,
    "127.0.0.1:4242",
)
```

Release automation is prepared to bind every platform artifact to its immutable
archive URL, SHA-256 digest, and Julia Git tree hash before packaging this
module.
