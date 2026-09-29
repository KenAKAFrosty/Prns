# Node-scoped handle and interface entropy

macOS arm64, based on `64171cb04`, 2026-09-28.

## Three review slices

1. A private synchronized owner wraps the existing core `RuntimeEntropy`.
   Cloning shares one stream rather than duplicating its state. Construction
   requires a successful OS seed; poisoned ownership refuses output.
2. Node handles retain the owner across clones and pass it through supervisor,
   fleet-member and direct-interface attachment. Unrelated nodes no longer
   consume a common executor-thread stream. Standalone seams and detached
   fleets own separate streams. The engine host's already-owned stream remains
   separate and does not gain a mutex.
3. Whole-value tests compare against the unchanged core generator, including
   concurrent 64-byte draws, cross-thread continuity, empty draws, seed failure,
   successful/failed reseeding and poisoned-lock refusal. A real attachment-path
   test observes four successive blocks through a handle clone, direct seam,
   supervisor and member seam, while an independent node retains its own first
   block. Fixed test seeds exist only under `cfg(test)` and switch back to the
   OS source before any reseed; no shipping seed switch was added.

The core already owns cryptographic generation and reseeding. The new Arc/mutex
owner belongs to the std Tokio adapter, not no-std core or Embassy. Public
constructors and wire formats are unchanged. `request_path` retains its direct
fallible OS call and `EntropyUnavailable` contract. Source injection across
all owners, deterministic restarts and packet-byte replay remain future work.

Production impact: node-scoped randomness replaces lazy thread-local ownership
for handles/interfaces. Seeding now happens at owner construction, even if no
bytes are subsequently requested. One shared allocation and pointer-sized
ownership references replace the zero-sized thread-local accessor; nonempty
draws acquire a short synchronous mutex. No lock spans an await. All clones
must be dropped before the shared owner is reclaimed. This is an ownership
correction for isolation/replay preparation, not an assertion that the previous
OS-backed generator produced insecure random bytes.

## Local cost probe

An ignored optimized unit-test probe reproduces the previous thread-local
`RefCell<Option<OsRuntimeEntropy>>` draw path and compares the node-owned path.
Both are warmed before measuring 100,000 fills at 16, 64 and 256 bytes, three
rounds each. It also prints owner/stream layout sizes. This is an uncontended
local microprobe, not a production throughput, contention, allocator-overhead
or fleet-memory benchmark. Source seeding is outside the measured interval;
normal core reseeding remains inside.

Observed 64-byte fills were 42.70–42.83 ns per call for the previous path and
47.47–47.52 ns for the owned path (roughly 4.7 ns, or 11%, extra in this local
uncontended operation). The 16-byte samples ranged 15.18–26.59 ns versus
17.92–32.68 ns; 256-byte samples ranged 161.96–325.99 ns versus 172.54–247.17 ns.
Those wider ranges were noisy under concurrent verification load; no speedup
or end-to-end slowdown is inferred from them. The engine-host draw path is
unchanged. Layout measurements were an 8-byte owner handle, 352 bytes for its
mutex plus generator, and two pointer-sized Arc counters (16 bytes), excluding
allocator overhead and any allocation-layout padding. These replace per-thread
retention with per-owner retention; they are not total per-node memory figures.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`:

```console
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib entropy --quiet
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-features --quiet
cargo clippy --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-features --all-targets -- -D warnings
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --release --lib compare_thread_local_and_owned_entropy_cost -- --ignored --nocapture
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation --suite integration-capstones
bash validation/hygiene/fmt-docs.sh
git diff --check
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
```

Runtime all-feature tests passed (275 tests, one ignored timing probe, and one
doc test), as did all-target clippy, root/workspace tests, repository formatting/
docs checks, tool/validation registry checks, website tests and `git diff --check`.
The registered simulation suite passed in 84.363 seconds. Integration capstones
initially failed to bind loopback sockets under the sandbox, then passed with
the required local-network permission in 12.756 seconds. The timing probe was
run explicitly and passed. Existing ignored tests remain ignored in ordinary
runs. The edited owners are outside the configured mutation-testing surface.
No firmware, hardware, other-host or full benchmark/publication lane was run;
this change is confined to the Tokio host adapter and does not alter core or
Embassy code.
