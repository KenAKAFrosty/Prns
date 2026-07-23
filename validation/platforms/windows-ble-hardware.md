# Windows BLE hardware release gate

This is the provisional manual gate for the WinRT Bluetooth Auto backend. It supplements the Windows CI lane until the project has hardware-in-the-loop Windows coverage.

## Equipment

- A Windows 11 or supported Windows 10 machine with a BLE adapter
- One independently powered Prns Bluetooth Auto peer
- A request-capable peer for the node-page transfer check

Record the Windows build, BLE adapter model and driver version, peer build, and commit SHA with the result.

## Build and capture

Run these commands from PowerShell at the repository root:

```powershell
cargo test --manifest-path prns-ffi/Cargo.toml --all-features --all-targets --locked
cargo clippy --manifest-path prns-ffi/Cargo.toml --all-targets --locked -- -D warnings
cargo build --manifest-path personal-hopspot/desktop/Cargo.toml --locked
$env:RUST_LOG = "debug"
cargo run --manifest-path personal-hopspot/desktop/Cargo.toml --locked 2>&1 |
  Tee-Object -FilePath windows-ble-hardware.log
```

## Required checks

1. Start Hopspot with Bluetooth enabled. The log must show the WinRT adapter ready, the GATT service published, advertising enabled, and scanning enabled. Adapter retries, publication failures, and watcher restart failures fail the gate.
2. Start the peer after Hopspot. Confirm Windows sights and dials it, the handshake completes, and the BLE card reports the member. Power-cycle the peer three times and confirm discovery and reconnection recover each time.
3. Reverse ownership so the peer dials the Windows node. Confirm the log reports an accepted inbound peer and both nodes retain the link.
4. While Windows owns the peripheral role, request `/page/index.mu` from its `nomadnetwork.node` destination. The complete page must arrive without notification, fragment decode, or reassembly failures. This exercises per-subscriber notification sizing with a multi-fragment response.
5. Toggle the BLE interface off, wait for the watcher to stop intentionally, then turn it on. Advertising and scanning must resume and the peer must reconnect without restarting Hopspot.
6. Leave the link active for 20 minutes while issuing announces and repeating the node-page request. Any disconnect loop, stalled transfer, watcher restart failure, or notification failure fails the gate.

If five peers are available, connect all five and confirm scanning stops at capacity without the watcher restarting itself. Disconnect one peer and confirm scanning resumes. Record this saturation check separately when the equipment is unavailable.

## Evidence

Attach `windows-ble-hardware.log` and record:

```text
Commit:
Windows build:
BLE adapter and driver:
Peer build:
Outbound role:
Inbound role:
Node-page transfer:
Disable/enable recovery:
20-minute soak:
Five-peer saturation:
Result:
```
