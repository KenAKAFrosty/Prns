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

The runtime supports ordinary process creation that starts a new program, including
`std::process::Command`. Continuing to use an inherited runtime after a raw Unix `fork` is not
supported. A forked child must execute a fresh program image before using Prns; continuing within
the inherited process could reuse cryptographic random-generator state from its parent.
