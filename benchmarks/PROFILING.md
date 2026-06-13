# Profiling the engine microscope

Three complementary tools for profiling the **pure-Prns** paths — the Rust engine
under `benches/engine_cycle.rs` and the scenario binaries (`scenario_node`,
`shaped_pipe`). These profile our own code; they have nothing to do with the
reference-implementation scenarios. Pick by the question you're asking:

| Tool | Question | Determinism | Install |
|------|----------|-------------|---------|
| **pprof + Criterion** | Where does wall-clock go inside a bench? (flamegraph) | sampled | built in (dev-dep) |
| **samply** | Let me explore the call tree / timeline interactively | sampled | `cargo install samply` |
| **iai-callgrind** | Exactly how many instructions per function, reproducibly? | deterministic | `cargo install iai-callgrind-runner` + `valgrind` |

All commands run from `benchmarks/`.

## 1. pprof flamegraphs — inside the Criterion run

The `engine_cycle` Criterion harness has a `PProfProfiler` attached, so its
`--profile-time` mode emits a flamegraph per benchmark (in-process sampling — no
`perf`, no sudo):

```sh
# one benchmark
cargo bench --bench engine_cycle -- --profile-time 10 single_cycle/roundtrip
# everything
cargo bench --bench engine_cycle -- --profile-time 10
```

Output: `target/criterion/<group>/<bench>/profile/flamegraph.svg`.

## 2. samply — interactive Firefox Profiler

Sampling profiler with a rich call-tree + timeline UI. On Linux it uses
`perf_event_open`; this host has `kernel.perf_event_paranoid = -1`, so **no sudo
needed**.

```sh
# a scenario binary (stable path)
cargo build --release --bin shaped_pipe
samply record -- ./target/release/shaped_pipe <args>

# the Criterion microscope (build first, then point samply at the binary)
cargo bench --bench engine_cycle --no-run
samply record -- "$(ls -t target/release/deps/engine_cycle-* | grep -vE '\.d$' | head -1)" \
    --bench single_cycle/roundtrip
```

`samply record` opens the profile in the browser. For headless capture, add
`--save-only -o profile.json.gz`, then `samply load profile.json.gz` later.

## 3. iai-callgrind — deterministic instruction counts

`benches/engine_cycle_iai.rs` runs the crypto primitives under Callgrind for
**reproducible** per-function instruction counts (no machine noise — ideal for
CI-trackable regression detection):

```sh
cargo bench --bench engine_cycle_iai
```

It prints instructions / cache hits / estimated cycles per primitive, and on
re-run reports the delta vs. the previous run (`N regressed`). Raw Callgrind
output lands under `target/iai/`; open it in KCachegrind or `callgrind_annotate`
for the full annotated call graph.

Covers both the crypto primitives (sign / verify / DH / seal / open) and the
engine-cycle stages (roundtrip / seal / deliver+prove / settle), via the shared
`Cycle` harness in `src/microscope.rs` — the same harness the Criterion bench
drives. Track the **instruction** counts for regressions (bit-exact run-to-run);
the estimated-cycle figures wobble slightly with cache state.
