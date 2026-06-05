# benchmarks

The performance harness the website's [Performance page](../docs/website/src/pages/benchmarks.rs)
promises: *"a deterministic harness in the repo, runnable on any machine."* A standalone
crate (own `[workspace]`, `publish = false`), mirroring `fuzz/` — so the engine's
`--workspace` gates never pull in `dhat`/`criterion`.

## The shape

One **scenario** is the constant; the **measurement backend** is the routable seam.

- `scenarios/<name>/` — each scenario's *input* as language-neutral data on disk (RNS wire
  packets + a manifest), so any implementation can replay the same bytes (see below).
- `src/lib.rs` — the scenario *code*: host-neutral, storage-generic workloads
  (`load_corpus`, `ingest_and_settle`, `tick_soak`, …). They measure nothing.
- `src/bin/` + `benches/` — the measurement backends that drive those scenarios.

That split is what lets the *same* workload run under dhat here, under criterion's timer,
and under `esp_alloc::HEAP.stats()` on the microcontroller when that route lands.

## The four axes (what the page commits to)

| Axis | Where | Status |
|------|-------|--------|
| **Memory** — peak footprint + allocation count (*the core makes none*) | `src/bin/mem_profile.rs`, `src/bin/mem_soak.rs` (dhat) | ✅ host route; the `FixedInline` path measures **0** allocations |
| **Throughput / Latency** — packets & bytes/sec, per-packet time | `benches/throughput.rs` (criterion) | 🟡 first bench landed |
| **Binary size** — what the engine costs on a constrained target | `scripts/binary-size.sh` (`cargo bloat` on the ESP32-C6 / riscv32imac firmware) | 🟡 engine ≈ **6.8 KiB** `.text` — crypto (sha2/curve25519/aes/ed25519) is the bulk |
| **Run on the hardware, down to the microcontroller** | the same scenarios + `esp_alloc::HEAP.stats()` in firmware | ⬜ next route |

## Other implementations (any language) — run it yourself

The page promises a harness "runnable on any machine" and comparison "against the RNS
reference where the comparison is fair." That only holds if the scenario is **data**, not
our API — so a scenario's *input* lives on disk as a versioned, language-neutral corpus:

```
scenarios/announce-256/
  manifest.json   # name, version, op sequence, and the expected end-state (the fairness gate)
  packets.hex     # one hex-encoded RNS wire packet per line — replay these exact bytes
```

`gen_corpus` regenerates it; every backend here `load_corpus`es it. **To participate, an
implementation writes a thin driver** that:

1. reads `packets.hex` + `manifest.json`,
2. feeds the packets through *its* engine per the manifest's `operations`,
3. checks it reaches the manifest's `expected` state — the **conformance gate**: only a
   matching impl's numbers are comparable, and
4. emits one result row per axis in a common schema:

   ```json
   {"scenario":"announce-256","scenario_version":1,"implementation":"personal-rns",
    "commit":"f987130","toolchain":"rustc 1.96.0","host":"aarch64-apple-darwin",
    "axis":"throughput","metric":"ingest_wall_ms","value":5.64,"unit":"ms"}
   ```

Our runners are the worked example. Measurement tooling stays per-language (you can't share
dhat with Python), so **throughput, binary size, and conformance compare cleanly across
implementations; memory and latency stay within-impl with loud caveats** — a cross-language
RSS race between a GC and a no-alloc core would be dishonest. The RNS 1.3.1 reference is the
first "other implementation" almost for free, since the engine is already wire-exact against it.

## Running

```sh
cargo run --release --bin gen_corpus      # (re)generate the scenario corpus on disk
cargo run --release --bin mem_profile     # static footprint + per-workload allocations
cargo run --release --bin mem_soak        # long-run tick soak (memory + state stay flat?)
MEM_SOAK_TICKS=50000000 MEM_SOAK_STEP_MS=100 cargo run --release --bin mem_soak
cargo bench                               # criterion throughput/latency
```

And the binary-size axis, from the repo root (it builds the ESP32-C6 firmware):

```sh
./scripts/binary-size.sh                  # the engine's .text share on a constrained target
```

The memory bins write `dhat-heap.json` (gitignored) for the [DHAT viewer](https://nnethercote.github.io/dh_view/dh_view.html).

## Methodology (the page's "how")

- **Deterministic** — fixed identities/entropy/clock; same inputs, same numbers.
- **Runnable on any machine** — pure Rust, no external profiler install for the memory axis.
- **Run on the hardware it claims** — the scenarios are storage-generic and reusable in
  firmware; the device route reports its own allocator's stats.
- **Reproducible** — report a figure with its commit + toolchain (`binary-size.sh` already
  stamps both; the other runners follow as the suite settles).
- **Honest** — figures land here as each axis stabilizes; empty slots stay empty until real.
