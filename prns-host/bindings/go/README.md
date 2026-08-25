# Personal RNS for Go

> **SDK preview: implemented, tested in-tree, and awaiting polished distribution.**
> This adapter runs the same Rust engine as every Prns node and is exercised by the repository's registered live Go conformance suite.
> Module tags, matching native archives, and public-package qualification are active release work, so a source checkout is currently the supported evaluation path.
> If you are experienced with Go API, cgo, or module design, help making this feel completely at home in Go would be especially valuable.

The Go module is a thin, typed adapter over the stable Personal RNS C capsule. Contract enums and sum types are generated from the same schema as Rust, TypeScript, .NET, Python, Swift, Kotlin, and Julia. Native waits are interrupted directly when a `context.Context` is cancelled, and application, diagnostic, and resource streams retain their single-consumer ownership.

## Evaluate the current source

On Linux, the registered suite builds a relocatable native capsule, exposes its pkg-config metadata, and runs the complete persistent two-node journey:

```console
python3 validation/run.py run --suite host-go-contract
```

The intended public delivery is an immutable Go module tag paired with a matching signed native archive. Until those artifacts have completed public qualification, do not assume the tag below exists. See the [SDK guide](../../../docs/sdks.md#native-sdk-previews) for the shared release posture and contribution path.

## API shape

With a matching native capsule's `lib/pkgconfig` directory visible through `PKG_CONFIG_PATH`, import:

```go
import prns "github.com/KenAKAFrosty/Prns/prns-host/bindings/go"

host, err := prns.NewHost(prns.EphemeralEndpoint(nil, []prns.Capability{
    prns.CapabilityTcpClient,
}))
if err != nil {
    return err
}
defer host.Close()

settlement, err := host.AttachTCPClient(
    ctx,
    "127.0.0.1:4242",
    prns.BitrateAuto{},
)
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

The module has no Go dependencies. Its eventual `0.3.7` tag uses the monorepo submodule form `prns-host/bindings/go/v0.3.7`.
