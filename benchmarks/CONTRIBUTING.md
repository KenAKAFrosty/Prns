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
| **Throughput / Latency** — packets & bytes/sec, per-packet time | `benches/throughput.rs` (criterion), `src/bin/sustained.rs` | ✅ sustained cross-impl throughput in [`RESULTS.md`](RESULTS.md) |
| **Energy** — joules per announce (the cross-comparable price a node pays) | `energy/` (`build.sh` + `sudo measure.sh`, powermetrics) | ✅ eight-impl energy table in [`RESULTS.md`](RESULTS.md); macOS now, Linux via RAPL next |
| **Run on the hardware, down to the microcontroller** | the same scenarios + `esp_alloc::HEAP.stats()` in firmware | ⬜ next route |

## Other implementations (any language) — run it yourself

The page promises a harness "runnable on any machine" and comparison "against the RNS
reference where the comparison is fair." That only holds if the scenario is **data**, not
our API — so a scenario's *input* lives on disk as a versioned, language-neutral corpus:

```
scenarios/announce-energy/
  manifest.json   # name, version, op sequence, and the expected end-state (the fairness gate)
  packets.hex     # one hex-encoded RNS wire packet per line — replay these exact bytes
```

The retired `announce-energy` corpus bytes were **minted by the RNS 1.3.5 reference**
(`reference/gen.py` drove the real `RNS.Destination.announce`), not by the engine-under-test.
They remain preserved in Git history as impartial provenance even though the scenario was removed
when the suite moved to realistic firehose workloads. RNS 1.4.0 revalidates that exact historical
blob byte-for-byte without restoring, regenerating, or relabeling it.

**Historical `announce-energy`** used 2560 distinct signed lxmf.delivery announces,
ingested under sustained all-cores load while CPU power is sampled, reported as **joules per
announce** — the cross-comparable price a battery/solar node actually pays, fair across
GC/JIT/interpreter because it's the actual energy. The announce path is ~97% independent
per-announce Ed25519 verify, so the ranking is a crypto-backend story; a port that can't make
its route store thread-safe is measured verify-only (no store), tagged ‡ in the table.

**To participate, an implementation writes a thin sustained harness** that:

1. reads `packets.hex`,
2. counts the routes it resolves / signatures it verifies in one pass — the **conformance
   gate** (only a matching impl's numbers are comparable) — and prints `CONFORMANCE resolved=N`,
3. then loops the corpus (replicated to a working set) across all logical cores for a fixed
   wall-time and prints `THROUGHPUT announces_per_sec=F`.

`energy/measure.sh` wraps `powermetrics` around that run and files one row per figure
(conformance, throughput, CPU power, energy) in a common schema:

   ```json
   {"scenario":"announce-energy","scenario_version":1,"implementation":"Prns",
    "commit":"5d132e7","toolchain":"1.96.0","host":"aarch64-apple-darwin",
    "axis":"energy","metric":"energy_microjoules_per_announce","value":67.5,"unit":"µJ/announce"}
   ```

Those rows are the **result substrate**: each implementation owns one file,
`results/<host>/<scenario>/<impl>.jsonl` (so two languages never contend on a write), where
`host` is the rustc target triple. `render_results` pivots every committed row into the
GitHub-facing table — an index ([`RESULTS.md`](RESULTS.md)) linking one **cross-implementation
comparison** per host, sorted by energy with the language, Ed25519 backend (the controlled
variable), conformance, sustained throughput, CPU power, and energy, plus a provenance list — and
the website **includes those same generated files**, so the numbers can't drift between the repo
and the site. `render_results --check` is the drift gate (re-render, diff, fail if stale), the
sibling of `gen_corpus --check`. The RNS reference is the worked second column:
`reference/sustained.py` replays the corpus through the real RNS announce path (`Packet.unpack`
+ `Identity.validate_announce`).

**The external ports' harnesses live in [`energy/contestants/`](energy/), built against pinned
upstream clones under `external/`.** Six more Reticulum ports — Rust
([Leviculum](https://codeberg.org/Lew_Palm/leviculum), [LXMF-rs](https://github.com/FreeTAKTeam/LXMF-rs)),
Go ([go-reticulum](https://github.com/svanichkin/go-reticulum)), Crystal
([rns-cr](https://github.com/jtippett/rns-cr)), C++
([microReticulum](https://github.com/attermann/microReticulum)), and a second Python
([RetiNet](https://codeberg.org/skyguy/retinet)). `energy/build.sh` clones each *pinned* upstream
into a gitignored `external/<impl>/.upstream/`, builds our sustained harness against it, and
self-tests; `sudo energy/measure.sh` samples power and files the rows. We never vendor upstream
source — only our harness and the measured numbers (licenses vary: AGPL, MIT, Apache, EPL, …).
What an implementation *is* — language, Ed25519 backend, repo, pinned ref, license — is
host-independent, so it lives once per impl in `implementations/<slug>.json` (the per-impl sibling
of `host.json`), which the table joins for its Language/backend columns and provenance.

**The host is a reproducibility dimension, not just a label.** A throughput number means nothing
without the silicon it ran on — an M1 and an M4 Max are both `aarch64-apple-darwin`. So each host
also carries a `results/<host>/host.json` descriptor (CPU model, core counts, memory, OS), written
once per machine by `cargo run --bin describe_host`. It's machine-level, not per-figure, so it lives
beside the rows rather than bloating each one; `render_results` renders it as the **Machine** block
atop that host's page. Run `describe_host` before committing a new host's results.

Our runners are the worked example. Measurement tooling stays per-language (you can't share
dhat with Python), so **throughput, energy, binary size, and conformance compare cleanly across
implementations; memory and latency stay within-impl with loud caveats** — a cross-language
RSS race between a GC and a no-alloc core would be dishonest. The active RNS 1.4.0 reference has
its own implementation descriptor for future runs. Historical RNS 1.3.5 measurements and their
generated pages retain their original labels (see [`RESULTS.md`](RESULTS.md)).

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
cargo run --release --bin render_results          # rebuild RESULTS.md + per-host pages from results/
cargo run --release --bin render_results -- --check  # is the table in sync with the substrate?
```

The cross-impl energy table (throughput + CPU power + joules/announce, all eight implementations)
needs root for the power counters, so it's its own two-step flow:

```sh
energy/build.sh             # clone pinned upstreams, build + self-test all 8 sustained harnesses (no sudo)
sudo energy/measure.sh 30   # idle baseline + a sampled run each -> results/<host>/announce-energy/*.jsonl
```

The historical announce-energy wire corpus remains the output minted by RNS 1.3.5. The current
reference venv installs RNS 1.4.0; release validation compares its generated bytes in memory with
the exact retired Git blob, without restoring or regenerating the worktree corpus. Use a modern
Python (3.12+); RNS's announce ingest is ~90% OpenSSL Ed25519 verification, so the interpreter
version barely moves the throughput number, but the reference deserves a current runtime rather
than a distro's EOL system Python (`brew install python@3.13`, or your platform's equivalent):

```sh
cd reference && python3.13 -m venv .venv && .venv/bin/pip install -r requirements.txt
```

The memory bins write `dhat-heap.json` (gitignored) for the [DHAT viewer](https://nnethercote.github.io/dh_view/dh_view.html).

## Methodology (the page's "how")

- **Deterministic** — fixed identities/entropy/clock; same inputs, same numbers.
- **Runnable on any machine** — pure Rust, no external profiler install for the memory axis.
- **Run on the hardware it claims** — the scenarios are storage-generic and reusable in
  firmware; the device route reports its own allocator's stats.
- **Reproducible** — report a figure with its commit + toolchain; the runners stamp both as
  the suite settles.
- **Honest** — figures land here as each axis stabilizes; empty slots stay empty until real.
