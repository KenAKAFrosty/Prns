# PersonalRns.jl

> **Status: solid core, young surface.**
> This binding runs the same Rust engine as every Prns node and passes the same cross-language conformance suite on every release.
> The young part is the Julia-facing API. Its shape is a working first draft: a starting point, not the final word.
> If you are an experienced Julia developer and something here does not feel native, that is exactly the feedback we want. Issues and PRs on API design are among the most valuable contributions right now.

`PersonalRns` is a thin Julia adapter over the stable native host capsule. Its concrete event, command, configuration, and outcome types are generated from the repository’s language-neutral contract. Julia multiple dispatch handles command cases directly, stream claims remain explicit values, and native readiness wakes Julia tasks through libuv without polling or occupying a worker thread.

Registry and release-source packages resolve the matching native artifact
automatically. Source-tree development can set `PRNS_HOST_LIBRARY` to an
explicit native capsule:

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

Release automation binds every platform artifact to its immutable archive URL,
SHA-256 digest, and Julia Git tree hash before packaging this module.
