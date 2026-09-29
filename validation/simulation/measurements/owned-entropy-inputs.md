# Explicit shared-stream and path-ID providers

## Construction and ownership

`PrnsNode::new_with_entropy_sources` requires the explicit host and an opaque
`TokioHandleEntropy` before building the recipe. The latter consumes an existing
core `RuntimeEntropy<S>` plus an independent fallible path-ID provider. Neither
generator policy nor reseed rules are reimplemented. Providers must be `Send`;
one `Arc`-owned mutex serializes access, and clones share the actual owner.
Ordinary constructors still initialize from the OS. No seed-valued production
API, test-mode feature or global random switch is added.

The shared owner is type-erased so handle, supervisor and interface contracts do
not acquire source generics. The engine host remains concrete, non-cloneable and
lock-free. A caller deliberately reusing one supplied owner across nodes shares
its state; normal node constructors continue to allocate independent owners.

Path IDs still perform a separate fallible provider read instead of drawing
from the infallible runtime generator. A source error or poisoned owner returns
`EntropyUnavailable` before timing lookup, command-ID allocation or admission.
No mutex guard crosses an await. A poisoned owner refuses runtime output too.

## Evidence

- Whole-stream tests retain continuous clone ownership, cross-thread uniqueness,
  independent owners, partial-stream handoff, empty fills, failed reseed and
  later recovery. Path reads do not advance the runtime generator.
- The attachment test now constructs a real node with supplied sources and
  consumes the first block during recipe construction, then checks handle,
  direct interface, supervisor and member-interface continuity.
- Existing path-failure/settlement tests now use the public `request_path` API
  and installed sources, including successive reads through handle clones.
- Replay fixtures now supply all three providers. A thin virtual-interface
  probe draws through the real seam before delegating to the unchanged frame
  medium. A dedicated scenario then draws through the handle, discovers a path
  through the public API and echoes a payload containing the handle draw.
  Exact observations match the core reference stream, and complete packet
  traces repeat. Changing the path source changes packets without changing
  the response; changing the shared stream changes both response and packets.

## Production impact and limits

This changes Tokio shared-source ownership and adds explicit construction APIs,
not the wire protocol or core RNG. OS-backed path reads now share the owner's
mutex, and poisoned ownership fails closed. Embedded runtime code is unchanged.
Standalone seams and detached fleets still use OS-backed ownership.

All three audited node-owned providers are selectable. This does not control
standard-library map hashing, process timestamps, application-provided sources,
native backend RNGs or worker completion order. BLE-specific replay and shared
provider lifecycle coverage remain forward work; the frame probe is not a BLE
controller emulator. Recorded traces remain private test values.

## Local cost probe

The ignored release probe compares the prior concrete
`Arc<Mutex<RuntimeEntropy<OsEntropySource>>>` fill path with the erased owner.
It also compares direct OS path-ID reads with owned reads. Five rounds perform
200,000 calls per case; the table reports median nanoseconds per call on macOS
arm64. The timing executable was built with the command below and rerun
unchanged after the parallel verification work finished. An earlier overlapping
run was noisy and is not used for these estimates.

| Operation | Previous | New |
| --- | ---: | ---: |
| Fill 16 bytes | 24.3 ns | 24.7 ns |
| Fill 64 bytes | 53.3 ns | 52.2 ns |
| Fill 256 bytes | 163.4 ns | 164.1 ns |
| OS path-ID read | 657.7 ns | 665.7 ns |

There is no notable fill regression in this small uncontended probe; variation
does not justify a speedup claim. The roughly 8-ns path-read difference is a
local estimate, not an end-to-end or contention result. OS costs and scheduling
vary. No fleet throughput, tail-latency or other-platform claim is made.

The owner reference grows from 8 to 16 bytes on this host. The default mutex
and state allocation payload remains 352 bytes, plus the same 16-byte Arc
counters (allocator metadata/padding excluded). Custom providers add their
own state. Each handle, fleet or seam carrying a reference pays the wider
pointer cost; it is not a new allocation per reference or per operation.

## Verification

Passed on macOS arm64; host Cargo invocations used `CARGO_INCREMENTAL=0`.

```console
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib entropy --quiet
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib path_discovery --quiet
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib handles_supervisors --quiet
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet entropy_replay --quiet
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-features --quiet
cargo clippy --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-features --all-targets -- -D warnings
cargo clippy --locked -p prns-simulation --features controlled-time --test manual_fleet -- -D warnings
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --release --lib compare_concrete_and_erased_ownership_cost -- --ignored --nocapture
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

Seven focused replay tests passed. Final Tokio coverage passed 281 tests and
one compile-fail doctest, with the timing probe ignored in the ordinary run.
The registered simulator suite passed in 99.002 seconds and integration
capstones in 39.655 seconds with local socket access. No mutation-selected owner
changed. Firmware, hardware, other operating systems and full benchmarks were
not run for this Tokio ownership change.
