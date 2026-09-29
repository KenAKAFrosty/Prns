# Explicit node hosts and bounded frame replay

## Construction and ownership

`PrnsNode` now retains its host's entropy source type, defaulting to
`OsEntropySource`. `new_with_host` and `new_with_handle_and_host` consume an
explicit `TokioHost<S>`; existing constructors retain their OS source and
persistence-derived or wall-clock timeline. Assembly has one implementation.
Persistence restore and configuration methods retain support for custom hosts.
There is no boxed source, added host lock or dynamic random-fill dispatch.

The constructor test checks that a partly consumed stream is not reset or
reseeded, that the recipe receives this node's handle, and that the supplied
clock advances correctly. The manual-fleet helper now constructs its host with
the scenario epoch directly instead of sampling wall time and overriding it.

## Frame-echo scenario, revision 1

Two real Tokio nodes run on the existing manual runner and bounded frame
medium. Fixed fixture identities, an explicit epoch, inline crypto and separate
scripted host sources drive an announce, link establishment and echo. Whole
medium event vectors include transmitted packet bytes and final shutdown;
there is no frame normalization. Source calls, exact application response,
actor cleanup, empty delivery queues, complete trace retention and matching
interface attachment/detachment are asserted.

Three fresh executions compare equal. Changing the host source changes the
frame trace; changing an equal-length application value changes it too. The
scripted sources exist only in this isolated test. The transcript is a private
Rust value, not a stable serialized replay artifact or cross-version golden.

## Limits and production impact

Normal shipping construction remains OS-backed with no intended behavior
change. The new API is additive and uses shared-core entropy unchanged.
Handle/interface and path-ID sources remain OS-backed even in this scenario.
The exercised frame interface does not draw entropy, and this echo path does
not request path IDs or call handle random-fill APIs. The supplied host owns
the randomness exercised by the packet-producing engine/inline crypto here.

This is evidence for this bounded scenario only: no BLE, transport forwarding,
restart, reseed, background-worker scheduling, automatic deadline discovery,
multi-interface inventory ordering or general full-node replay claim is made.
The complete entropy-source design remains unfinished.

## Verification

Passed on macOS arm64. Host Cargo commands used `CARGO_INCREMENTAL=0`.

```console
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib explicit_host --quiet
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet entropy_replay --quiet
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-features --quiet
cargo clippy --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-features --all-targets -- -D warnings
cargo clippy --locked -p prns-simulation --features controlled-time --test manual_fleet -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
python3 validation/run.py run --suite integration-capstones
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

The focused constructor test and both frame-replay tests passed. Tokio's
all-feature run passed 282 tests and one compile-fail doctest, with one ignored
timing probe. Registered simulation passed in 106.682 seconds; integration
capstones passed in 51.780 seconds with local socket access. These durations
are validation wall time, not performance measurements.

No changed owner is selected by the mutation configuration. No firmware,
physical hardware, other operating-system builds or benchmarks were run for
this host-adapter and simulator change.
