# Personal RNS (Prns)

This crate is one package in the Personal RNS public Rust graph. Quick overviews, the complete feature guide, API documentation, examples, and the cross-language SDK overview are available at [prns.dev](https://prns.dev) or [reticulum.rs](https://reticulum.rs), and in the [source repository](https://github.com/KenAKAFrosty/Prns).

All public packages use the same engine, release version, and dual MIT/Apache-2.0 license.

## Interface replacement

Interface inventory follows attachment instances, not just stable interface IDs.
When a stopped interface is replaced before its queued teardown runs, retirement
must preserve the replacement's status and count only the departing instance's
traffic. The driver retains that instance's existing attachment epoch and shared
status view until retirement; it does not allocate a second status object.

The focused lifecycle regressions cover queued same-ID replacement, independent
status retirement, final byte/frame accounting, and run teardown ordering:

```console
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib interface_lifecycle
```

This is status-lifetime protection for ordered teardown and reattachment, not a
claim that arbitrary concurrent same-ID attachments or stale attachment handles
are interchangeable.

## Request admission

The shared core's typed request transport plan separates inactive or missing
links from Resource-sized requests. Tokio settles the former as
`SendRequestFailure::Rejected` immediately. Previously they entered the Resource
path, whose rejection did not settle the request waiter.

The mixed Tokio/Embassy BLE regression exercises expired-link requests and
successful exchanges on fresh links:

```console
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble interop
```

Resource upload failures now settle the request waiter with
`SendRequestFailure::RequestTransferFailed`. Shared core retires its exact receipt,
suppresses late duplicate results, and waits for the response after upload proof.
The mixed-runtime regression also checks prompt peer rejection of an oversized
request, followed by a successful ordinary exchange on the same link.

## Process model

Handle and interface randomness belongs to the node, not the executor thread.
Handle clones, supervisors and attached interface seams share one synchronized,
OS-seeded stream; cloning ownership never copies generator state. Independent
nodes and detached fleets have independent owners. Standalone interface seams
construct their own owner. Construction eagerly seeds the stream and refuses
to proceed if the OS seed source fails; a poisoned stream also fails closed.
The engine host retains its separate, unsynchronized owned stream. Both use
the shared core generator and reseeding policy. Path-discovery identifiers
still use their existing fallible direct OS call.

This ownership boundary does not expose deterministic entropy in production
and is not full replay support. It adds one shared allocation per owner and a
short mutex hold per nonempty handle/interface draw; empty fills are inert.
Tests cover clone continuity, thread transfer, concurrent block consumption,
node isolation, actual attachment plumbing and core reseed semantics.

The runtime supports ordinary process creation that starts a new program, including
`std::process::Command`. Continuing to use an inherited runtime after a raw Unix `fork` is not
supported. A forked child must execute a fresh program image before using Prns; continuing within
the inherited process could reuse cryptographic random-generator state from its parent.

## Hosts without native 64-bit atomics

The Tokio runtime uses `portable-atomic` for 64-bit counters, packed status values,
command IDs, and interface attachment epochs. Native atomic operations remain
available on supported targets; other hosts, including 32-bit MIPS Linux, use the
crate's synchronized fallback. Values remain 64-bit and existing memory orderings
are preserved. The fallback is not guaranteed to be lock-free. Linux hosts do not
enable interrupt-disabling or single-core assumptions.

## Controlled workers for validation

The nondefault `simulation-control` feature exposes `ControlledCrypto` through
`CryptoPoolConfig::Controlled`. Ordinary builds contain neither the control
handle nor worker trace hooks. Validation supplies explicit worker and trace
budgets; the process-wide crypto environment override cannot replace this mode.

Controlled workers use the production worker selection, admission accounting,
job/result rings, crypto functions, completion readiness and manifold dispatch.
Execution and publication can be held separately by work kind. A transition
executes one real job; native verification/signing batches and actual thread
handoffs retain their separate worker tests. A retired control cannot attach to
a replacement node. Queue, computed-result, published-result and trace occupancy
have finite bounds, and trace overflow fails qualification instead of evicting
evidence. The simulator interleaves worker transitions with actor polls without
advancing its clock.
