# benchmarks

The performance harness the website's [Performance page](../docs/website/src/pages/benchmarks.rs)
promises: *"a deterministic harness in the repo, runnable on any machine."* A standalone
crate (own `[workspace]`, `publish = false`), mirroring `fuzz/` — so the engine's
`--workspace` gates never pull in `dhat`/`criterion`.

## The shape

One **scenario** is the constant; the **measurement backend** is the routable seam.

- `src/lib.rs` — the scenarios: host-neutral, storage-generic workloads
  (`announce_fixtures`, `ingest_and_settle`, `tick_soak`, …). They measure nothing.
- `src/bin/` + `benches/` — the measurement backends that drive those scenarios.

That split is what lets the *same* workload run under dhat here, under criterion's timer,
and under `esp_alloc::HEAP.stats()` on the microcontroller when that route lands.

## The four axes (what the page commits to)

| Axis | Where | Status |
|------|-------|--------|
| **Memory** — peak footprint + allocation count (*the core makes none*) | `src/bin/mem_profile.rs`, `src/bin/mem_soak.rs` (dhat) | ✅ host route; the `FixedCapacity` path measures **0** allocations |
| **Throughput / Latency** — packets & bytes/sec, per-packet time | `benches/throughput.rs` (criterion) | 🟡 first bench landed |
| **Binary size** — what the engine costs on a constrained target | `cargo size` / `cargo bloat` on a host firmware (script TBD) | ⬜ slot |
| **Run on the hardware, down to the microcontroller** | the same scenarios + `esp_alloc::HEAP.stats()` in firmware | ⬜ next route |

## Running

```sh
cargo run --release --bin mem_profile     # static footprint + per-workload allocations
cargo run --release --bin mem_soak        # long-run tick soak (memory + state stay flat?)
MEM_SOAK_TICKS=50000000 MEM_SOAK_STEP_MS=100 cargo run --release --bin mem_soak
cargo bench                               # criterion throughput/latency
```

The memory bins write `dhat-heap.json` (gitignored) for the [DHAT viewer](https://nnethercote.github.io/dh_view/dh_view.html).

## Methodology (the page's "how")

- **Deterministic** — fixed identities/entropy/clock; same inputs, same numbers.
- **Runnable on any machine** — pure Rust, no external profiler install for the memory axis.
- **Run on the hardware it claims** — the scenarios are storage-generic and reusable in
  firmware; the device route reports its own allocator's stats.
- **Reproducible** — report a figure with its commit + toolchain (a stamp the runners
  will print as the suite settles).
- **Honest** — figures land here as each axis stabilizes; empty slots stay empty until real.
