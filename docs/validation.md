# Validation Lanes

## Reference Target

Wire and transport parity work targets Reticulum `1.3.1` through completion,
using upstream commit `1d7cfe7c202c5e2f3cd7a9d70fa2a6c8c6848958` as the
stable source reference. Later Reticulum releases should not change these
vectors or predicates unless the parity target is intentionally moved.

The normal workspace tests stay the first pass:

```sh
cargo test --workspace
```

The extra lanes below are intentionally small. They set the pattern for deeper
coverage without making every local edit pay the full proof, fuzzing, and
mutation-testing cost.

## Property Tests

Property tests live beside the modules they exercise and run with the ordinary
`personal-rns` test suite:

```sh
cargo test -p personal-rns
```

Current seed coverage:

- `wire`: arbitrary typed packet headers must write and parse back to the same
  value.
- `interfaces::rns_serial_framing`: arbitrary payloads must round-trip through
  the RNS reference-compatible byte-stuffed serial framing, including arbitrary
  stream chunk boundaries.

## Kani Proofs

Kani harnesses are gated behind `cfg(kani)`, so normal builds do not compile
them. Run the focused proofs with:

```sh
cargo kani -p personal-rns --harness hops_above_pathfinder_m_always_reject_before_any_other_gate
cargo kani -p personal-rns --harness an_upstream_app_destination_rejects_when_hops_are_in_range
cargo kani -p personal-rns --harness reemit_announce_exact_buffer_serializes_header_and_payload_length
cargo kani -p personal-rns --harness reemit_announce_short_buffer_rejects_before_a_full_packet_is_written
cargo kani -p personal-rns --harness decoded_signalling_bytes_always_land_in_range
cargo kani -p personal-rns --harness signalling_bytes_round_trip_for_every_in_range_mtu_and_mode
cargo kani -p personal-rns --harness keepalive_for_any_rtt_stays_inside_the_reference_clamp
cargo kani -p personal-rns --harness stale_is_exactly_twice_any_clamped_keepalive
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
  overflow.

## Fuzzing

The cargo-fuzz package is isolated under `fuzz/` so it stays out of the normal
workspace build. Use a nightly toolchain:

```sh
cargo +nightly fuzz check
cargo +nightly fuzz run wire_announce_parse -- -max_total_time=30
cargo +nightly fuzz run egress_reemit_round_trip -- -max_total_time=30
cargo +nightly fuzz run link_handshake_parse -- -max_total_time=30
cargo +nightly fuzz run engine_ingest_never_panics -- -max_total_time=30
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
  pinned RNS 1.3.1 handshake vectors.
- `engine_ingest_never_panics`: the deterministic core's whole inbound edge.
  Each input is split into length-prefixed frames fed sequentially into
  `EngineState::ingest_packet_into` on a two-interface engine with a registered
  destination and request handler, so cross-packet state (announce then link
  request) is reachable. The engine must never panic on any inbound byte
  sequence; reactions are deliberately discarded.

## Mutation Testing

`cargo-mutants` reads `.cargo/mutants.toml` from the source-tree root. The
checked-in config narrows the lane to contract-heavy surfaces: wire parsing,
RNS serial framing, interface capabilities and interface-set storage, typed
ingress/egress, self-announce scheduling, engine directive buffers, routing
defaults, announce IDs, announce acceptance, held-announce caches, rebroadcast
queues, link handshake framing, link maintenance math, the request/response
codec, delivery receipts, and Ed25519 signing:

```sh
cargo mutants --list-files
cargo mutants
```

Treat survivors as review prompts, not automatic failures, until the team has
triaged enough runs to decide which mutants are equivalent and which are true
coverage gaps.

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
