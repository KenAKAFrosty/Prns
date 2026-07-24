# Prns C host ABI

This crate is the stable binary capsule beneath native language bindings. Rust backends publish semantic events through `HostPublisher`; foreign runtimes see only opaque host, event-stream, event, and resource-stream handles from `include/prns_host.h`.

The header is generated from `prns-host/schema/host-contract-v1.json`. Run `./tools/prns run repo.host-contract.generate` after an intentional schema change and `./tools/prns run repo.host-contract.check` in review or release automation.

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
- All entry points contain Rust panics and return `PRNS_STATUS_PANIC` where a status can be returned.

Host and stream operations are safe from multiple native threads. An individual event or resource handle must not be released while another thread is reading it.

## Versioning

Product version and contract ABI are separate gates. A language package must require both before creating a host. Additive schema work uses unused discriminants and tail fields in size-prefixed structures. Removing a case, changing a discriminant, changing ownership, changing a field’s meaning, or reusing a reserved value requires a new contract ABI.

`prns-host/conformance/host-contract-v1.json` is the portable oracle for fixed sizes, limits, discriminants, and mismatch behavior. Rust tests additionally exercise lifecycle terminality, pressure, exact diagnostic gaps, single ownership, event-view lifetimes, and resource transfer.

## Build

```sh
cargo build --manifest-path prns-host/abi/c/Cargo.toml --release
cc -std=c11 -Wall -Wextra -Werror -fsyntax-only prns-host/abi/c/tests/header-smoke.c
```

The produced library is `prns_host` on each platform. Language packages should ship the matching target binary beside their managed adapter and let the platform loader select it.
