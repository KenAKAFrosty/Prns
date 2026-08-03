# Prns C host ABI

> **SDK preview: implemented, tested in-tree, and awaiting polished distribution.**
> The engine beneath this ABI is the same Rust core every Prns node runs, and the generated header is exercised as both C11 and C++17 by the repository's registered live conformance suite.
> Signed native archives, installer ergonomics, and public-package qualification are active release work, so a source checkout is currently the supported evaluation path.
> If you are experienced with C or C++ API and package design, help making this feel completely at home in those ecosystems would be especially valuable.

This crate is the stable binary capsule beneath native language bindings. Rust backends publish semantic events through `HostPublisher`; foreign runtimes see only opaque host, event-stream, event, and resource-stream handles from `include/prns_host.h`.

The header is generated from `prns-host/schema/host-contract-v1.json`. Run `./tools/prns run repo.host-contract.generate` after an intentional schema change and `./tools/prns run repo.host-contract.check` in review or release automation.

## Evaluate the current source

On Linux, the registered suite builds the native capsule, compiles the same
persistent two-node consumer as C11 and C++17, and runs both binaries:

```console
python3 validation/run.py run --suite host-c-contract
```

The intended public delivery is a signed target archive containing the header,
dynamic and static libraries, pkg-config metadata, checksums, and licenses.
Until those archives have completed public qualification, build the capsule
from this checkout. See the [SDK guide](../../../docs/sdks.md#native-sdk-previews)
for the shared release posture and contribution path.

## Mechanical contract

- Every enum and tagged case has a permanent unsigned discriminant.
- Every public structure starts with `struct_size`. A caller initializes it to `sizeof(structure)`; the callee rejects undersized input and can accept larger future structures.
- Rust enums, slices, strings, allocators, futures, and unwinding never cross the boundary.
- Handles are opaque, owned, and released exactly once with their matching release function.
- Output pointers are non-null, writable for their declared result, and do not overlap live inputs or other outputs from the same call.
- An event byte or string view is borrowed from its event handle and remains valid until that event is released.
- A resource chunk view remains valid until the next operation on that resource stream or until the stream is released.
- Input views are borrowed only for the duration of the call.
- A stream claim has one owner. A second claim returns `PRNS_STATUS_ALREADY_CLAIMED`; releasing the first stream returns the claim to its host.
- Application events remain lossless within configured count and byte bounds. Exceeding either bound fails the host explicitly.
- Diagnostics may drop newest and later produce one `DiagnosticsDropped` event with the exact accumulated `uint128` count.
- `prns_event_stream_next` is pull-based. Zero milliseconds is nonblocking, `UINT32_MAX` waits indefinitely, and every finite nonzero timeout is bounded.
- Commands and event streams each accept one readiness registration. Registration observes subsequent changes; consumers first inspect the source nonblockingly, then wait for a hint whenever it cannot make progress.
- Readiness callbacks carry no event or settlement data. They schedule the foreign runtime’s waiter and return; consumers retain authority by calling `prns_command_wait` or `prns_event_stream_next` with a zero timeout.
- Wake hints may be coalesced or spurious. A consumer drains until the source reports timed out or would block, then waits for another hint.
- Releasing a readiness registration or its source waits for an in-flight callback to return. The callback must not release its own registration or source.
- All entry points contain Rust panics and return `PRNS_STATUS_PANIC` where a status can be returned.

Host, command, and stream operations are safe from multiple native threads. An individual event or resource handle must not be released while another thread is reading it.

## Versioning

Product version, schema version, and C ABI are three explicit creation gates.
The first public baseline is product `0.3.1`, schema 1, ABI 1. The capsule has
one `PrnsHostOptions` layout and no compatibility shim for unpublished earlier
layouts. `struct_size` remains on public structures so every call can prove the
memory prefix it may read or write; undersized structures are rejected and
larger structures are accepted at their known prefix. That safety mechanism is
not a promise to preserve pre-baseline layouts.

The schema's operation IDL generates every exported declaration. Each
`HostCommand` case becomes its own `prns_host_*` function, matching ordinary C
calling conventions and debugger/tooling expectations. Ownership, borrowed
lifetimes, readiness, interruption, and release relationships are validated
before the header is rendered.

`prns-host/conformance/host-contract-v1.json` is the portable oracle for fixed sizes, limits, discriminants, and mismatch behavior. Rust tests additionally exercise lifecycle terminality, pressure, exact diagnostic gaps, single ownership, event-view lifetimes, and resource transfer.

## Build

```sh
cargo build --manifest-path prns-host/abi/c/Cargo.toml --release
cc -std=c11 -Wall -Wextra -Werror -fsyntax-only prns-host/abi/c/tests/header-smoke.c
```

The produced library is `prns_host` on each platform. Language packages should ship the matching target binary beside their managed adapter and let the platform loader select it.
