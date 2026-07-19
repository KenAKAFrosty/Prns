# Validation Lanes

## Reference Target

Wire, transport, and the shared-instance data plane target Reticulum `1.3.5`
semantics through completion. Daemon configuration, management destinations,
and the local shared-instance control RPC separately target RNS `1.3.8`
semantics. Both oracle installs run `rns==1.3.9` (security update), pinned in
`benchmarks/reference/requirements.txt` and
`benchmarks/reference/rpc-requirements.txt`. Older pickle-shaped control RPC
remains a compatibility fallback for legacy clients, but it is not the primary
RPC parity target.

Daemon configuration semantics additionally track the RNS `1.3.8` `internal`
mode, recursive path-request forwarding, and internal-announcement controls.
This does not change the broader wire and transport pin above.

The normal workspace tests stay the first pass:

```sh
cargo test --workspace
```

The extra lanes below are intentionally small. They set the pattern for deeper
coverage without making every local edit pay the full proof, fuzzing, and
mutation-testing cost.

## Drift Guard

The validation docs name executable targets, so they get their own cheap drift
guard. It checks the active RNS pin, documented fuzz targets, documented Kani
harnesses, and mutation-lane paths against the repo:

```sh
bash scripts/validation-doc-drift.sh
```

## Deep Validation

For release hardening or architecture changes, run the operator lane. It layers
the drift guard, focused tests, local/tcp feature tests, the 1.3.8 local RPC
oracle, mutation file-list sanity, Kani proofs, cargo-fuzz checks, and a
validation artifact manifest into one entrypoint:

```sh
bash scripts/deep-validation.sh
```

Use `--quick` for a cheap local shape check, and `--mutants` or `--android` when
you intentionally want the full mutation lane or attached-device Android
foreground-service runtime smoke folded in.

The manual GitHub workflow can run the same lane on demand. A scheduled nightly
run exercises the long-form fuzz/Kani/mutation surface and uploads the retained
evidence: fuzz crash artifacts, fuzz corpora, mutation output, and a manifest
under `validation-artifacts/`.

## Memory, Leak, and Race Sanitizers

The hardening lane instruments the standard library and the Prns test builds on
Linux x86_64 with nightly Rust. It covers `prns-core`, the Tokio-host runtime,
the cross-crate integration suite, and every Tokio interface feature:

```sh
bash scripts/sanitizers.sh address
bash scripts/sanitizers.sh leak
bash scripts/sanitizers.sh thread
bash scripts/sanitizers.sh all
```

AddressSanitizer checks executed native code for invalid memory access;
LeakSanitizer reports allocations that become unreachable; ThreadSanitizer
checks executed synchronization for data races. The builds use an explicit
`x86_64-unknown-linux-gnu` target and `-Zbuild-std`, so Rust's standard library
is instrumented along with the project. `-Copt-level=2` with debug information
keeps symbols while avoiding debug-only dilation of protocol tests with real
wall-clock deadlines. The lane runs library and integration-test targets;
rustdoc does not inherit the sanitizer instrumentation, and linking its
uninstrumented doctest crate to the instrumented standard library is rejected
as an ABI mismatch.

Tests are serialized at the harness boundary to remove unrelated cross-test
noise; Tokio runtimes and worker threads inside each test remain concurrent.

AddressSanitizer runs with its bundled leak detector disabled because the
standalone leak lane owns that signal. ThreadSanitizer has one narrow dependency
suppression in `scripts/tsan-suppressions.txt` for Tokio's
`runtime::io::scheduled_io::ScheduledIo`. Rust's sanitizer currently lacks the
`fcntl(F_DUPFD_CLOEXEC)` interceptor Tokio needs, which produces the same
initialization report upstream; the lane prints suppression match counts and
still fails on every unsuppressed report. Track the upstream limitation in
[`rust-lang/rust#130037`](https://github.com/rust-lang/rust/issues/130037).

These flags affect only the hardening test build. They add no code, dependency,
or runtime cost to normal host or embedded artifacts. The workflow runs each
sanitizer as a separate weekly and manually dispatchable job so one failure
does not hide the other two reports.

## Miri

The default Miri lane is a curated, isolated 95-test pass over parser and wire
boundaries, fixed-capacity indexed storage, resource tables, Bluetooth framing,
streaming token open, identity crypto, and streamed resources:

```sh
bash scripts/miri.sh
bash scripts/miri.sh --quick
bash scripts/miri.sh --full
bash scripts/miri.sh --stacked wire::tests
bash scripts/miri.sh --tree identity::in_memory::tests
```

The quick lane uses Miri's default Stacked Borrows model where the dependency
stack accepts it and Tree Borrows for the in-place RustCrypto paths. At present,
the default model rejects an `inout` AES/CBC aliasing pattern while the same
tests pass under Tree Borrows; keeping the split visible avoids globally
weakening the model or disguising the dependency boundary. `--full` runs all
`prns-core` tests under Tree Borrows, while `--stacked` and `--tree` make either
model available for a focused filter or a deliberate full run.

Host isolation remains enabled. Property tests keep 32 generated cases but
disable on-disk failure persistence, because that persistence asks Miri for the
host working directory. Miri validates only paths the selected tests execute;
it complements rather than replaces Kani proofs or native sanitizer coverage.
Like the sanitizers, it changes no production artifact.

## Unsafe Dependency Inventory

Install the pinned audit tool and run the source-entrypoint and dependency
views separately:

```sh
cargo install cargo-geiger --version 0.13.0 --locked
bash scripts/geiger.sh --entrypoints
bash scripts/geiger.sh --inventory
bash scripts/geiger.sh --all
```

The entrypoint view makes the intended boundary visible: the engine, runtime,
facade, and Tokio interfaces forbid unsafe code, while `prns-ffi` is the
deliberately quarantined platform-FFI exception. The inventory view enumerates
transitive unsafe exposure in crypto, allocation, OS, and runtime dependencies.
It scans the default facade, all Tokio interface features, and the host-visible
FFI graph independently; the facade's mutually exclusive runtime and keyring
features make one global `--all-features` graph invalid.

`cargo-geiger` 0.13.0 currently emits package-source matching warnings and has
parser failures on sources in this graph, including `nb 0.1.3` and
`signal-hook-registry 1.4.8`. Its full inventory can therefore be incomplete
and exit nonzero after producing a useful partial report. The weekly workflow
preserves that advisory report but does not turn incompleteness into a false
green security claim. The compiler's `forbid(unsafe_code)` remains the
enforcement boundary for safe Prns crates; geiger is inventory and review
evidence, not a proof of soundness.

## Instrumentation Boundary

The observability workline is compile-time isolated from the checks in this
hardening lane. Tokio hosts can opt into structured tracing and fixed runtime
counters, while Embassy and `no_std` builds retain their existing engine and
hot-loop boundary. The signal policy, feature graph, exporter bounds, privacy
rules, and local demonstration live in [Observability](observability.md).

## Property Tests

Property tests live beside the modules they exercise and run with the ordinary
`personal-rns` test suite:

```sh
cargo test -p personal-rns
```

Current seed coverage:

- `wire`: arbitrary typed packet headers must write and parse back to the same
  value.
- `interfaces::ifac`: arbitrary open-header payloads, across every accepted
  IFAC size and rejection boundary, must mask and unmask back to the original
  packet under the same access context.
- `interfaces::rns_serial_framing`: arbitrary payloads must round-trip through
  the RNS reference-compatible byte-stuffed serial framing, including arbitrary
  stream chunk boundaries.

## Kani Proofs

Kani harnesses are gated behind `cfg(kani)`, so normal builds do not compile
them. Run the focused proofs with:

```sh
cargo kani -p prns-core --harness hops_above_pathfinder_m_always_reject_before_any_other_gate
cargo kani -p prns-core --harness an_upstream_app_destination_rejects_when_hops_are_in_range
cargo kani -p prns-core --harness reemit_announce_exact_buffer_serializes_header_and_payload_length
cargo kani -p prns-core --harness reemit_announce_short_buffer_rejects_before_a_full_packet_is_written
cargo kani -p prns-core --harness decoded_signalling_bytes_always_land_in_range
cargo kani -p prns-core --harness signalling_bytes_round_trip_for_every_in_range_mtu_and_mode
cargo kani -p prns-core --harness keepalive_for_any_rtt_stays_inside_the_reference_clamp
cargo kani -p prns-core --harness stale_is_exactly_twice_any_clamped_keepalive
cargo kani -p prns-core --harness the_grace_never_dips_below_the_stale_grace_floor
cargo kani -p prns-core --harness fleet_member_and_supervisor_kinds_are_inverses
cargo kani -p prns-core --harness fleet_supervisor_discriminants_fit_the_fan_mask
cargo kani -p prns-core --harness proof_plaintext_round_trips_for_any_hash_pair
cargo kani -p prns-core --harness cancel_plaintext_round_trips_for_any_resource_hash
cargo kani -p prns-core --harness an_anchored_radio_never_retunes_and_never_seeks
cargo kani -p prns-core --harness a_free_radio_never_stays
cargo kani -p prns-core --harness two_radios_that_have_learned_each_other_always_converge
cargo kani -p prns-core --harness a_channel_that_cannot_host_a_group_always_yields_incompatible
cargo kani -p prns-core --harness path_request_parse_never_panics_for_any_wire_payload
```

Current proof coverage:

- `announce::acceptance`: max-hop rejection wins before later gates, and
  upstream app destinations reject once hops are in range.
- `engine::egress`: a re-emitted announce with an exact buffer always produces
  a well-formed transport announce packet carrying the via transport id, and a
  one-byte-short buffer rejects before claiming a full packet was written.
- `links::handshake`: any three signalling bytes decode to an in-range MTU and
  mode bits, and every in-range MTU/mode pair survives the encode/decode round
  trip.
- `links::maintenance`: any RTT yields a keepalive inside the reference clamp
  `[5_000, 360_000]`, and staleness is exactly twice that keepalive with no
  overflow; the timeout grace never drops under the stale-grace floor.
- `interfaces::kind`: every fleet supervisor/member kind pair is a two-way
  inverse, including the Android/local `LocalServer` -> `LocalClient` lane, and
  every supervisor discriminant fits the u128 announce-fan mask shift.
- `links::resources::control`: resource proof and cancel plaintexts round-trip
  for any 32-byte resource/proof hash pair.

## Android Foreground Service Smoke

The Android face must package a foreground `PrnsService` that owns the local
shared-instance server and exposes a signature-protected bind action for other
apps on the device. The package smoke builds both shipped JNI ABIs, assembles
the debug APK, verifies the service contract in the merged manifest, and
confirms both native libraries are packaged:

```sh
bash scripts/android-service-smoke.sh
```

The runtime smoke requires an attached Android device or emulator. It installs
the debug APK plus a same-signature instrumentation probe, starts the foreground
service from that separate test package, sends HOME to background the app,
binds through `org.personal.hopspot.action.BIND_PRNS_CLIENT`, and asserts the
client-facing status bundle reports the local shared-instance ports plus the
production health shape: foreground state, instance role, RPC port, service and
runtime uptime, bound-client count, interface totals, route/link totals, traffic
totals, and live transfer rates.

```sh
bash scripts/android-runtime-smoke.sh
```

The stable `MSG_STATUS` Bundle keys are:

`state`, `running`, `foreground`, `instance_role`, `local_port`, `rpc_port`,
`rpc_key_hex`, `service_uptime_ms`, `runtime_uptime_ms`, `client_count`,
`interface_count`, `online_interface_count`, `local_client_count`,
`route_count`, `link_count`, `transported_link_count`, `rx_bytes`, `tx_bytes`,
`rx_bps`, `tx_bps`, and optionally `last_error`.

The same-signature client binding contract is documented in
[`docs/android-shared-instance-client.md`](android-shared-instance-client.md).

## RNS 1.3.8 Daemon Oracles

The shared-instance, management-service, and host-interface promises are checked against stock
RNS `1.3.8` at the client API boundary. Prepare their dedicated reference
environment without changing the broader 1.3.5 target:

```sh
uv venv benchmarks/reference/.rpc-venv
uv pip install --python benchmarks/reference/.rpc-venv/bin/python -r benchmarks/reference/rpc-requirements.txt
```

Then run the oracles:

```sh
bash scripts/local-rpc-interop-smoke.sh
bash scripts/remote-management-interop-smoke.sh
bash scripts/probe-responder-interop-smoke.sh
bash scripts/blackhole-exchange-interop-smoke.sh
bash scripts/rnode-tcp-interop-smoke.sh
```

The first smoke stands up a Prns-owned shared instance, lets a stock RNS client
join it, and calls Reticulum's own `get_*` methods. Those methods issue msgpack
control-RPC requests in 1.3.8 and decode msgpack replies. The second authenticates
to `rnstransport.remote.management` and exercises the stock status, path, and
rate request forms. The third sends an ordinary packet to `rnstransport.probe`
and requires a cryptographically valid delivery proof. The fourth runs both directions of the
blackhole exchange: stock RNS fetches Prnsd's published aggregate, then Prnsd fetches and persists a
stock RNS source list. The fifth uses the RNS 1.3.8 RNode command constants in a Python TCP device
oracle and verifies Prnsd's detect, radio configuration, report validation, and idle keepalive bytes.

The compatibility shim still answers legacy pickle-shaped basics so older LXMF
clients do not fault on startup or resource/link telemetry, but full RPC parity
tracks the 1.3.8 msgpack contract. The current oracle covers all 21 operations:

- Live-shaped reads: interface stats, link count, path table, rate table,
  next-hop hash/name, first-hop timeout, and packet RSSI/SNR/Q.
- Management reads: blackholed identity table and `is_blackholed`.
- Conservative management writes: unknown path drops, all-via drops, announce
  queue drops, identity blackhole/unblackhole, destination retain/use/unretain,
  and identity retain. These return typed no-op values until backed by real
  engine state, so stock clients do not fault and Prns does not claim fake
  mutation.

## IFAC TCP Interop Oracle

The protected TCP lane puts a Prns TCP client and a stock RNS `1.3.5` TCP
server on the same named IFAC network with a 16-byte access code. Two stock RNS
applications then establish links through that protected interface and transfer
one-megabyte resources in both directions, crossing the mask counter wrap and
the broadcast MTU boundary:

```sh
bash scripts/ifac-tcp-interop-smoke.sh
```

The ordinary open-interface transit smoke remains available as
`scripts/local-transit-smoke.sh`.

## Fuzzing

The cargo-fuzz package is isolated under `fuzz/` so it stays out of the normal
workspace build. Use a nightly toolchain:

```sh
cargo +nightly fuzz check
cargo +nightly fuzz run wire_announce_parse -- -max_total_time=30
cargo +nightly fuzz run egress_reemit_round_trip -- -max_total_time=30
cargo +nightly fuzz run link_handshake_parse -- -max_total_time=30
cargo +nightly fuzz run engine_ingest_never_panics -- -max_total_time=30
cargo +nightly fuzz run config_configobj_parse -- -max_total_time=30
cargo +nightly fuzz run config_reference_parse -- -max_total_time=30
cargo +nightly fuzz run resource_plaintexts_parse -- -max_total_time=30
cargo +nightly fuzz run shared_instance_rpc_request_msgpack -- -max_total_time=30
```

Current targets:

- `wire_announce_parse`: arbitrary bytes enter the wire parser; any parsed
  header is re-encoded, and announce-shaped payloads are passed through announce
  validation. The corpus includes a real RNS announce vector as a hex seed.
- `egress_reemit_round_trip`: fuzzed hop counts, via transport ids, targets, and
  output slack exercise re-emitted real announces. Serialization must preserve
  the announce payload, produce the expected transport header carrying the via,
  retain the engine-named target interface, and reject one-byte-short buffers.
- `link_handshake_parse`: arbitrary bytes enter the three link establishment
  parsers an open network can reach - `parse_link_request` (unsigned, so every
  byte is attacker-controlled), `validate_link_proof` against a fixed responder
  key, and `parse_link_rtt` against a fixed link key. The corpus seeds are the
  pinned RNS 1.3.5 handshake vectors.
- `engine_ingest_never_panics`: the deterministic core's whole inbound edge.
  Each input drives a sequence of 16-bit-length-prefixed frames, absolute and
  saturating logical-time changes, interface departures and reattachments, and
  scheduled-deadline firings on a two-interface engine with a registered
  destination and request handler. Cross-packet and cross-lifecycle state is
  retained throughout; the engine must never panic on any operation or inbound
  byte sequence, and reactions are deliberately discarded.
- `config_configobj_parse` / `config_reference_parse`: arbitrary config bytes
  enter the in-tree and reference-style parsers so parser drift does not hide
  behind the ordinary happy-path config fixtures.
- `resource_plaintexts_parse`: arbitrary resource advertisement, hashmap
  update, part-request, proof, and cancel plaintexts enter the exposed parsers.
  Any parsed, writable shape is serialized and parsed again to pin the codec
  boundary without requiring production behavior changes.
- `shared_instance_rpc_request_msgpack`: arbitrary bytes enter the private RNS
  1.3.8 request decoder; malformed, contradictory, truncated, and unknown
  messages must return a typed error without panicking.

## Mutation Testing

`cargo-mutants` reads `.cargo/mutants.toml` from the source-tree root. The
checked-in config runs `personal-rns` with the `local tcp` host feature surface
because the lane includes the shared-instance RPC shim. It narrows mutation to
contract-heavy surfaces: wire parsing, IFAC masking, RNS serial framing, local
shared-instance RPC encoding/dispatch, interface capabilities and runtime
interface storage, typed inbound/egress edges, app commands, engine reactions,
scheduled/tick work, announce defaults, announce IDs, announce acceptance,
held-announce caches, scheduled-announce queues, link handshake framing, link
maintenance math, the request/response codec, resource-control plaintexts,
delivery receipts, and Ed25519 signing:

```sh
cargo mutants --list-files
cargo mutants
```

For full runs, prefer the triage wrapper; it preserves `mutants.out` and prints
the missed/timeout/unviable counts with the first survivor names:

```sh
bash scripts/mutation-triage.sh
```

Treat survivors as review prompts, not automatic failures, until the team has
triaged enough runs to decide which mutants are equivalent and which are true
coverage gaps. The Links-era triage worked exactly this way: a first run over
the link surfaces left 45 survivors, which resolved into boundary tests (every
wire writer now has an exact-fit/one-byte-short pair), per-gate malformed
vectors for the request/response parsers, an active-link MDU test for the
commanded send/respond paths, a reference-minted vector for the pre-signalling
link proof older peers send, and a handful of genuine equivalents — some
excluded with justification, some made unrepresentable by hoisting a shared
length const or dropping a guard already subsumed by the saturating cast.

## Local Build Cleanup

Standalone UI, fuzz, Android, and embedded host builds keep their own ignored
artifact trees. Use the dry run first when local discovery or disk usage gets
noisy:

```sh
sh scripts/clean-local-builds.sh
sh scripts/clean-local-builds.sh --apply
```

The script only targets ignored build outputs and prints what it sees before
removing anything.
