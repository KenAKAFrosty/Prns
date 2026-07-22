# Profiling the engine microscope

Five complementary tools for profiling the **pure-Prns** paths — the Rust engine
under `benches/engine_cycle.rs` and the `participant_node` scenario binary.
These profile our own code; they have nothing to do with the
reference-implementation scenarios. Pick by the question you're asking:

| Tool | Question | Determinism | Install |
|------|----------|-------------|---------|
| **pprof + Criterion** | Where does wall-clock go inside a bench? (flamegraph) | sampled | built in (dev-dep) |
| **resource_profile** | Is bulk resource transfer engine-bound, and which resource stage owns it? | staged wall-clock | built in |
| **samply** | Let me explore the call tree / timeline interactively | sampled | `cargo install samply` |
| **iai-callgrind** | Exactly how many instructions / cache hits / branches per function, reproducibly? | deterministic | `cargo install iai-callgrind-runner` + `valgrind` |
| **dhat** | How many heap allocations / bytes per operation? Which call sites? | deterministic | built in (dev-dep) |

All commands run from `benchmarks/`.

## Resource transfer split — engine-only bulk resources

`resource_profile` drives real resource sends across an already-established link
inside two live engines, with no TCP, tokio, Python, or scenario process glue. It
prints the full resource frame mix plus stage timing for sender advertise,
receiver pull, sender serve, receiver assemble, and proof settlement:

```sh
RUSTFLAGS="-C target-cpu=native --cfg aes_armv8" cargo build --release --bin resource_profile
./target/release/resource_profile 256 1048575 8
```

Use this before cutting into resource code: if this number is much higher than a
live scenario, the next bottleneck is outside the pure engine path.

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
cargo bench --features callgrind --bench engine_cycle_iai
```

It prints instructions / cache hits / estimated cycles per primitive, and on
re-run reports the delta vs. the previous run (`N regressed`). Raw Callgrind
output lands under `target/iai/`; open it in KCachegrind or `callgrind_annotate`
for the full annotated call graph.

Covers the crypto primitives (sign / verify / DH / seal / open), the engine-cycle
stages (roundtrip / seal / deliver+prove / settle) via the shared `Cycle` harness,
the HDLC framing both ways (`framing_encode` / `framing_decode` — the SWAR
escape-scan on both sides), and the transport relay (`relay_forward` — a batch of
distinct SINGLEs switched through the `Forward` harness). Track the **instruction**
counts for regressions (bit-exact run-to-run); the estimated-cycle figures wobble
slightly with cache state.

## 4. dhat — heap allocations per operation

`examples/dhat_*.rs` measure the *other* axis: not cycles but **allocations**.
Each example owns its own `#[global_allocator] = dhat::Alloc`, so the
instrumented allocator only exists inside that one example binary — the lib, iai,
and Criterion paths are never perturbed. Same `Cycle` harness.

```sh
# endpoint SINGLE roundtrip — allocations-per-roundtrip (testing mode, instant)
cargo run --release --example dhat_cycle
# relay forward path — allocations-per-forward (initiator seal kept out of the window)
cargo run --release --example dhat_forward
# either example: dump dhat-heap.json for the call-site viewer
cargo run --release --example dhat_cycle heap
```

The readout reports allocation **blocks** and **bytes** per operation (delta over
N cycles, post-warmup), live-block flatness (a non-zero delta that doesn't grow
across runs is retention, not a leak), and peak live. The `heap` arg writes
`dhat-heap.json` — open it at <https://nnethercote.github.io/dh_view/dh_view.html>
to rank allocation call sites by total bytes/blocks and drill the stack of each.

Steady-state findings:
- **Endpoint SINGLE roundtrip** (`dhat_cycle`): ~0.01 allocs/cycle — the crypto
  seal/open/prove/verify path allocates nothing per cycle; the only heap traffic
  is the dedup history's two `Vec`s doubling as they fill (`Generation::insert` +
  index resize), amortized and bounded by rotate-on-full.
- **Relay forward path** (`dhat_forward`): ~0.025 allocs/forward — a transport
  node switches blind ciphertext with **no per-packet allocation**; its only heap
  traffic is amortized growth of the dedup, reverse-route, and receipt stores.

Both invariants are gated under `tests/` (`forward_path_alloc`,
`dedup_rotation_alloc`): a per-packet-allocation regression turns the
handful-of-blocks figure into one-block-per-packet and trips the assertion. Use
these to hold a **no-per-packet-allocation** line on the hot paths as they grow.
