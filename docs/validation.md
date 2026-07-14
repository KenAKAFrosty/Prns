# Validation Lanes

## Reference Target

Wire, transport, and shared-instance parity target Reticulum `1.3.5` through
completion. The active reference install is pinned in
`benchmarks/reference/requirements.txt`, and the local shared-instance RPC
contract follows the 1.3.5 msgpack control plane. Older pickle-shaped control
RPC remains a compatibility fallback for legacy clients, but it is not the
primary parity target.

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
the drift guard, focused tests, local/tcp feature tests, the 1.3.5 local RPC
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

## Local Shared-Instance RPC Oracle

The local server/client promise is checked against stock RNS `1.3.5` at the
client API boundary. The smoke stands up a Prns-owned shared instance, lets a
stock RNS client join it, and calls Reticulum's own `get_*` methods. Those
methods issue msgpack control-RPC requests in 1.3.5 and decode msgpack replies:

```sh
bash scripts/local-rpc-interop-smoke.sh
```

The compatibility shim still answers legacy pickle-shaped basics so older LXMF
clients do not fault on startup or resource/link telemetry, but full RPC parity
tracks the 1.3.5 msgpack contract. The current oracle covers:

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
cargo +nightly fuzz run shared_instance_rpc_value_msgpack -- -max_total_time=30
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
  Each input is split into length-prefixed frames fed sequentially into
  `EngineState::ingest_packet_into` on a two-interface engine with a registered
  destination and request handler, so cross-packet state (announce then link
  request) is reachable. The engine must never panic on any inbound byte
  sequence; reactions are deliberately discarded.
- `config_configobj_parse` / `config_reference_parse`: arbitrary config bytes
  enter the in-tree and reference-style parsers so parser drift does not hide
  behind the ordinary happy-path config fixtures.
- `resource_plaintexts_parse`: arbitrary resource advertisement, hashmap
  update, part-request, proof, and cancel plaintexts enter the exposed parsers.
  Any parsed, writable shape is serialized and parsed again to pin the codec
  boundary without requiring production behavior changes.
- `shared_instance_rpc_value_msgpack`: arbitrary bounded control-RPC reply trees enter
  the shared-instance msgpack encoder used by `LocalServer`'s RPC compatibility
  shim; encoding must be non-empty and deterministic.

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
