# Local BLE connection-index measurement

This is a focused development measurement, not a qualified benchmark publication,
production-node capacity result, or release performance claim.

The comparison uses `b8d997fd4992e69a6602d141e0629343656ccd59` with the new
`ble_fleet` probe added, versus the uncommitted per-radio connection-index slice
on that checkpoint. Both run the identical probe, dependencies, default Cargo
test profile (unoptimized), and Rust 1.98.0 on the same macOS ARM64 host.

```console
cargo test --locked -p prns-simulation --test ble_fleet measure_fleet_connection_churn -- --ignored --exact --nocapture
```

Each sample starts with the requested number of independent, sparse BLE pairs
already connected. It times 512 replacements: capacity refusal, alternating pair
and radio disconnects, closed-endpoint checks, and reconnecting while old handles
still exist. No sleeping, sockets, hardware, production-node buffers, or runtime
deadline acceleration participate. Construction, discovery, full-fleet counting,
and subsequent exact data-delivery/cleanup assertions are outside the timed
region. Each row has three separately constructed fleets, run serially.

Elapsed milliseconds for the same 512 replacements:

| Live pairs | Global index samples | Per-radio index samples |
| --- | --- | --- |
| 32 | 6.552, 6.339, 6.316 | 4.463, 4.787, 4.577 |
| 256 | 29.822, 30.006, 28.740 | 5.378, 5.100, 5.382 |
| 2,048 | 201.524, 201.612, 199.473 | 5.893, 5.716, 5.620 |

The result supports removing fleet-wide scans from connection-local operations.
It does not establish end-to-end runtime throughput, memory per production node,
or behavior under dense reachability. There is no timing threshold in CI.

The new index stores two references per connection and a map entry/vector per
participating radio, rather than one reference in a fleet-wide vector. Radio
admission budgets bound retained references, including closed entries waiting for
local cleanup. This is a host-memory tradeoff, not a zero-cost optimization; RSS
and allocator overhead were not measured here. Full-fleet connection counting
remains an explicit linear inspection.
