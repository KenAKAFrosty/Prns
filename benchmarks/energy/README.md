# Energy per announce (`announce-energy`)

The cross-comparable axis: **joules per announce**. Throughput has GC/RSS caveats, but energy
is the bottom-line price a user pays — and on a battery/solar mesh node it *is* the product.
`J/announce = (active CPU power − idle baseline) ÷ throughput`, fair across every runtime
regardless of GC/JIT/interpreter, because it's the actual joules paid.

Each contestant does **one sustained all-cores run** that yields both throughput and power: the
workload pegs the CPU for the window (looping a replicated 50k working set), `powermetrics`
integrates package power over it from a separate root process (negligible load), and we divide.
Throughput here is the sustained *average* under continuous load — the energy denominator.
Python runs all-core threads but is GIL-bound, so its all-cores ≈ one core (the honest outcome
of asking Python to use the machine).

## Run

```sh
./build.sh                 # no sudo: clones the pinned upstreams, builds + self-tests all 8 harnesses
sudo ./measure.sh 30       # sudo: idle baseline + a sampled run each → results/<host>/announce-energy/*.jsonl
cargo run --release --bin render_results   # rewrite the tables
```

`measure.sh` samples power for `<secs>`; 30s windows ≈ 5 min total, `60` for steadier numbers. It
files rows owned by `$SUDO_USER`. Each harness measures conformance in one pass before the
sustained loop (printing `CONFORMANCE resolved=N`), so the table's conformance column is a real
per-impl gate, not an assertion.

## Why this needs root (and isn't a one-command driver)

`powermetrics` (macOS) reads privileged CPU power counters, so unlike the corpus/render drivers
this can't be a no-auth reproduce — it's a documented `sudo` step, macOS-only for now.

- **Linux:** `perf stat -e power/energy-pkg/,power/energy-cores/ -- <cmd>` (RAPL) returns joules
  directly; swap it into `measure.sh`.
- **Embedded (the real prize):** an inline USB power meter or an INA219/INA260 shunt on the board
  rail measures actual mW during ingest. That's where J/announce *matters* — a solar LoRa node's
  battery life is literally this number — and where a `no_std` core should crush a GC'd runtime.

## Files

- `../src/bin/sustained.rs` (ours) and `../reference/sustained.py` (RNS; RetiNet reuses it).
- `contestants/<impl>/` — sustained harnesses for the six external ports, built against pinned
  `external/<impl>/.upstream/` clones that `build.sh` fetches.
- `build.sh` (build + self-test) and `measure.sh` (sample + file rows).
