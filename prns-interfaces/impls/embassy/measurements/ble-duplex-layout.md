# T114 BLE duplex future layout

The comparison is against the exact prior implementation at
`b903ad7a67a832e3609f2a38d9f39c561ec2a34f`, using Rust 1.98.0 / LLVM 22.1.8 on
macOS ARM64 and the registered T114 resource recipe. Both builds use the same
five BLE peers, existing packet buffers, S140 6.1.1, configured fat LTO, and
68 KiB minimum runtime stack reservation.

```console
./tools/prns build embedded resources report --target t114
```

The baseline fails the linker stack guard. Its emitted linker map still records
the main task pool and static-memory end, allowing a comparison without
disabling that guard. The changed implementation borrows caller-pinned work and
receive futures, avoiding nested owning async adapters while retaining the
shared receive-length validator.

| Linker quantity | Prior implementation | Borrowed work |
| --- | ---: | ---: |
| Main task pool, bytes | 28,696 | 27,640 |
| Static RAM end | `0x2002f2b8` | `0x2002ee98` |
| RAM above static allocation, bytes | 68,936 | 69,992 |
| Margin above the 69,632-byte stack reservation | -696 | 360 |

This recovers 1,056 bytes of static RAM on this firmware build. It does not
measure peak runtime stack, RF behavior, total BLE memory, or throughput. The
remaining margin above the reserved stack is small; it is not a new allocation
budget. Compiler versions and backend future types can change the layout, so
the real firmware resource gate remains authoritative.

The core layout regression checks that enlarging the borrowed work state does
not enlarge the receive driver's future. Behavioral tests separately cover
work-first ordering, receive and forwarding failures, checked lengths, empty
frames, exact settlement, fanout receive progress, and cancellation accounting.

```console
cargo test --locked -p prns-core interfaces::bluetooth_auto::duplex
python3 validation/run.py run --suite bluetooth-auto-embassy --suite virtual-device-simulation
```
