# Binding implementation guide

Every native binding is an adapter over `prns_host.h`, not a second host
implementation. This constraint is what keeps many ecosystem packages one
system.

## Required shape

1. Generate the language's constants, fixed-size values, enums, and closed
   unions from `host-contract-v1.json`.
2. Load the matching native capsule and verify contract ABI, schema version, and
   product version before host creation.
3. Translate semantic configuration into size-prefixed C structures whose
   backing memory lives through the call and is zeroed when it held a secret.
4. Wrap every opaque pointer in one deterministic owner.
5. Model command settlement as success-or-failure data. A protocol failure is
   not an exception; misuse, contract mismatch, and native status failure are.
6. Expose application and diagnostic events as the ecosystem's native
   single-consumer stream abstraction.
7. Register command and event readiness with the ecosystem’s scheduler, drain
   the source nonblockingly once after registration and after each wake, and
   wire cancellation directly to `prns_command_interrupt_wait` or
   `prns_event_stream_interrupt_wait`.
   Release the readiness registration before its callback context and release
   the source handle only after any active drain returns.
8. Decode every generated event case. Unknown discriminants fail loudly instead
   of becoming lossy maps.
9. Marshal every case in `../conformance/interface-configs-v2.json` through the
   language adapter without requiring the corresponding hardware.
10. Run the shared `../conformance/persistent-two-node-v2.json` journey over a
   real loopback TCP connection: persist two hosts, attach, announce and
   discover, establish a link, request and respond, transfer the resource from
   its listed chunks through the bounded upload path, stop, restart, and verify
   restored identities, destination, route, and persistence status.
11. Keep the lifecycle, exclusive-stream-claim, cancellation, and generic
    interface attach/detach checks alongside that journey.
12. Report platform support from the host's runtime `BackendInfo`. Static
    package documentation may explain how to query it, but must not substitute
    a hard-coded capability matrix for the runtime result.
13. Package the adapter with an exact compatible native target or an explicit,
    documented native archive dependency.

## Next low-friction ecosystems

The stable C surface already makes C++ and Zig usable without another runtime:
C++ can include the generated header, while Zig can import it and build a thin
error-union/iterator layer.

The next small adapters should be:

| Ecosystem | Natural mapping | Package |
| --- | --- | --- |
| Dart and Flutter | sealed classes, `Stream<T>`, `Finalizer`, `dart:ffi` | pub package plus Android/iOS/desktop native assets |
| Ruby | immutable variant objects, `Enumerable`, interruptible worker | gem using Fiddle or ffi |
| PHP | readonly variant classes, generator, explicit `close()` | Composer package using FFI |
| Lua/LuaJIT | tagged tables and coroutine iterator | LuaRocks package over FFI |
| R | S3/S4 variants and external pointers with finalizers | CRAN source package over `.Call`/C |
| Haskell | algebraic data types, `Conduit`/`Streamly`, bracketed `ForeignPtr` | Hackage package |

Dart is the best next mobile return because the release matrix already supplies
Android and Apple/desktop libraries. Ruby is the smallest dynamic-language port.
Zig and C++ are the smallest systems-language layers. None needs a new ABI.

New language projections belong in
`tools/repo/generate-host-contract.py`; hand-maintained discriminants do not.
Convenience helpers come after the raw semantic surface and conformance smoke,
and must compile down to the same bounded, interruptible happy path.
