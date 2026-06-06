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
| **Throughput / Latency** — packets & bytes/sec, per-packet time | `benches/throughput.rs` (criterion), `src/bin/bench_result.rs` | 🟡 criterion bench landed; first cross-impl row (ours vs RNS) in [`RESULTS.md`](RESULTS.md) |
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

The bytes are **minted by the RNS 1.3.1 reference** (`reference/gen.py` drives the real
`RNS.Destination.announce`), not by the engine-under-test — so the corpus is impartial
ground truth and doubles as a conformance corpus. Our engine reproduces it **byte-for-byte**
(`cargo run --bin gen_corpus -- --check` vs `reference/gen.py --check`), which is the
first hard proof of wire-exactness against RNS. Every backend here `load_corpus`es it.

**To participate, an implementation writes a thin driver** that:

1. reads `packets.hex` + `manifest.json`,
2. feeds the packets through *its* engine per the manifest's `operations`,
3. checks it reaches the manifest's `expected` state — the **conformance gate**: only a
   matching impl's numbers are comparable, and
4. emits one result row per axis in a common schema:

   ```json
   {"scenario":"announce-256","scenario_version":1,"implementation":"personal-rns",
    "commit":"f400e59","toolchain":"1.96.0","host":"aarch64-apple-darwin",
    "axis":"throughput","metric":"ingest_announces_per_sec","value":47897.5,"unit":"announce/s"}
   ```

Those rows are the **result substrate**: each implementation owns one file,
`results/<host>/<scenario>/<impl>.jsonl` (so two languages never contend on a write), where
`host` is the rustc target triple. `render_results` pivots every committed row into the
GitHub-facing tables — an index ([`RESULTS.md`](RESULTS.md)) linking one **cross-implementation
comparison** per host, each sorted by ingest throughput with the language, Ed25519 backend,
conformance, and speed relative to the RNS reference (`×ref`), plus a provenance list — and the
website **includes those same generated files**, so the numbers can't drift between the repo and
the site. `render_results --check` is the drift gate (re-render, diff, fail if stale), the sibling
of `gen_corpus --check`. The RNS reference is the worked second column: `reference/driver.py`
replays the corpus through the real RNS announce path (`Packet.unpack` + `Identity.validate_announce`)
and emits its rows.

**Other implementations live in [`external/`](external/).** Six more Reticulum ports — Rust
([Leviculum](external/leviculum), [LXMF-rs](external/lxmf-rs)), Go
([go-reticulum](external/go-reticulum)), Crystal ([rns-cr](external/rns-cr)), C++
([microReticulum](external/microreticulum)), and a second Python ([RetiNet](external/retinet)) —
each get an `external/<impl>/` with our harness, a README, and a **one-command `run.sh`** that
clones the *pinned* upstream into a gitignored `.upstream/`, builds the harness against it, and
files rows in the schema above (`cd benchmarks && ./external/leviculum/run.sh`). We never vendor
upstream source — only our harness and the measured numbers (licenses vary: AGPL, MIT, Apache,
EPL, …). What an implementation *is* — language, Ed25519 backend, repo, pinned ref, license — is
host-independent, so it lives once per impl in `implementations/<slug>.json` (the per-impl sibling
of `host.json`), which the comparison table joins for its Language/backend columns and provenance.
To add one, see [`external/README.md`](external/README.md).

**The host is a reproducibility dimension, not just a label.** A throughput number means nothing
without the silicon it ran on — an M1 and an M4 Max are both `aarch64-apple-darwin`. So each host
also carries a `results/<host>/host.json` descriptor (CPU model, core counts, memory, OS), written
once per machine by `cargo run --bin describe_host`. It's machine-level, not per-figure, so it lives
beside the rows rather than bloating each one; `render_results` renders it as the **Machine** block
atop that host's page. Run `describe_host` before committing a new host's results.

Our runners are the worked example. Measurement tooling stays per-language (you can't share
dhat with Python), so **throughput, binary size, and conformance compare cleanly across
implementations; memory and latency stay within-impl with loud caveats** — a cross-language
RSS race between a GC and a no-alloc core would be dishonest. The RNS 1.3.1 reference is
already the first "other implementation": it mints the corpus (`reference/gen.py`), the engine
reproduces it byte-for-byte, and both resolve every route on ingest (see [`RESULTS.md`](RESULTS.md)).

One honest parity nuance the byte-diff surfaced: RNS fills `random_hash`'s trailing 5 bytes
with `int(time.time())` (unix **seconds**); our engine writes its `now` (**milliseconds**)
there. The field is opaque dedup entropy to receivers, so interop is unaffected — but our
*live* announces aren't byte-identical to RNS's in that field. The corpus pins both sides to
the same nonce, so the conformance diff stays exact.

## Running

```sh
cargo run --release --bin gen_corpus -- --check   # engine parity: ours == the committed corpus
cargo run --release --bin gen_corpus              # write manifest.json (+ bootstrap packets if absent)
cargo run --release --bin mem_profile             # static footprint + per-workload allocations
cargo run --release --bin mem_soak                # long-run tick soak (memory + state stay flat?)
MEM_SOAK_TICKS=50000000 MEM_SOAK_STEP_MS=100 cargo run --release --bin mem_soak
cargo bench                                       # criterion throughput/latency
cargo run --release --bin describe_host           # record this machine -> results/<host>/host.json
cargo run --release --bin bench_result            # measure ours -> results/<host>/announce-256/personal-rns.jsonl
cargo run --release --bin render_results          # rebuild RESULTS.md + per-host pages from results/
cargo run --release --bin render_results -- --check  # is the table in sync with the substrate?
```

The canonical wire corpus is minted from the RNS 1.3.1 reference (one-time venv setup). Use a
modern Python (3.12+); RNS's announce ingest is ~90% OpenSSL Ed25519 verification, so the
interpreter version barely moves the throughput number, but the reference deserves a current
runtime rather than a distro's EOL system Python (`brew install python@3.13`, or your platform's
equivalent):

```sh
cd reference && python3.13 -m venv .venv && .venv/bin/pip install -r requirements.txt
.venv/bin/python gen.py --check           # reference parity: RNS == the committed corpus
.venv/bin/python gen.py                    # regenerate packets.hex from RNS (canonical)
.venv/bin/python driver.py                 # measure RNS -> results/<host>/announce-256/rns-1.3.1.jsonl
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
