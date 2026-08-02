# Personal RNS for Go

> **Status: solid core, young surface.**
> This binding runs the same Rust engine as every Prns node and passes the same cross-language conformance suite on every release.
> The young part is the Go-facing API. Its shape is a working first draft: a starting point, not the final word.
> If you are an experienced Go developer and something here does not feel native, that is exactly the feedback we want. Issues and PRs on API design are among the most valuable contributions right now.

The Go module is a thin, typed adapter over the stable Personal RNS C capsule. Contract enums and sum types are generated from the same schema as Rust, TypeScript, .NET, Python, Swift, Kotlin, and Julia. Native waits are interrupted directly when a `context.Context` is cancelled, and application, diagnostic, and resource streams retain their single-consumer ownership.

Install a matching native capsule and make its `lib/pkgconfig` directory visible through `PKG_CONFIG_PATH`, then import:

```go
import prns "github.com/KenAKAFrosty/Prns/prns-host/bindings/go"

host, err := prns.NewHost(prns.EphemeralEndpoint(nil, []prns.Capability{
    prns.CapabilityTcpClient,
}))
if err != nil {
    return err
}
defer host.Close()

command, err := host.Execute(prns.HostCommandAttachTcpClient{
    Target: "127.0.0.1:4242",
    Bitrate: prns.BitrateAuto{},
})
if err != nil {
    return err
}
defer command.Close()

settlement, err := command.Wait(ctx)
if err != nil {
    return err
}
switch value := settlement.(type) {
case prns.CommandSucceeded:
    handle(value.Outcome)
case prns.CommandFailed:
    handleFailure(value.Failure)
}
```

The module has no Go dependencies. Its version tag must use the monorepo submodule form `prns-host/bindings/go/v0.3.2`.
