# Local BLE peer receive-buffer measurement

This is a focused development measurement of one concrete Rust future, not RSS,
allocator overhead, a full-node capacity result, or a throughput benchmark.

The baseline is `353d9ca2fc25b06cbd7fb0a47f63db4f700fdf8b` with the
`ble_peer_memory` probe added. The comparison is the uncommitted receive-buffer
slice on that checkpoint. Both use the same idle source, sink, and seam types,
root workspace features, default unoptimized test profile, Rust 1.98.0, and
macOS ARM64 host.

```console
cargo test --locked -p prns-simulation --test ble_peer_memory measure_bluetooth_peer_future -- --ignored --exact --nocapture
```

The probe constructs the production `BluetoothPeer::run` future and reports
`size_of_val` before polling. Its source and sink do not allocate, and its seam
holds an empty vector. The future's layout includes the inline receive array
and this fixture's state; native source/sink implementations have other state.

| Component | Baseline bytes | Transport-sized bytes |
| --- | ---: | ---: |
| Inline receive array | 524,352 | 564 |
| Measured peer future | 525,056 | 1,264 |

The measured future is 523,792 bytes smaller. This removes the global
`MAX_WIRE_FRAME_LEN` allocation from every Tokio BLE peer, including simulated
peers, without assuming that all node memory is accounted for here.

The receive capacity is **not** the 500-byte packet MTU alone. The manifold
reserves up to 64 additional interface-authentication bytes. `BLE_WIRE_FRAME_LEN`
derives from those existing constants, and a test compares it with
`frame_cap_for` on the canonical BLE descriptor.

Before tightening the buffer, the native and Embassy source copies were changed
to reject insufficient output capacity instead of returning a truncated frame.
Linux L2CAP also checks the declared frame length before waiting for its body.
The peer validates returned lengths before accounting or slicing, so a faulty
backend cannot panic it by reporting a length beyond the supplied buffer.
Reassembly limits and negotiated transport capabilities are otherwise unchanged;
this is not an expansion of every adapter's supported wire-frame sizes.

Automated tests cover whole-buffer refusal, full wire-size input/output,
authentication headroom, empty input, invalid reported lengths, stream-prefix
refusal and retry, and a generous future-size ceiling independent of exact
compiler padding. The manual probe remains ignored in ordinary test runs.

## Local verification

The focused commands are:

```console
cargo test --locked -p prns-core interfaces::bluetooth_auto::receive
cargo test --locked -p prns-simulation --test ble_peer_frames --test ble_peer_memory
cargo test --locked --manifest-path prns-interfaces/impls/tokio/Cargo.toml --features bluetooth-auto --lib bluetooth_auto
cargo test --locked --manifest-path prns-ffi/Cargo.toml --target-dir prns-interfaces/impls/tokio/target --lib bluetooth_auto
cargo check --locked --manifest-path prns-interfaces/impls/embassy/Cargo.toml --features bluetooth-auto-trouble
python3 validation/run.py run --suite virtual-device-simulation
```

The FFI command exercises native macOS and host-runnable Android implementation
tests, not an Android device. Linux and Windows adapter tests require their
respective hosts and were not run here. No radio hardware or complete firmware
build is claimed; embedded buffer capacities are unchanged.
