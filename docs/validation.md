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
cargo kani -p personal-rns --harness local_destination_rejects_when_hops_are_in_range
cargo kani -p personal-rns --harness reemit_announce_exact_buffer_serializes_header_and_payload_length
cargo kani -p personal-rns --harness reemit_announce_short_buffer_rejects_before_a_full_packet_is_written
```

Current proof coverage:

- `announce::acceptance`: max-hop rejection wins before later gates, and local
  destinations reject once hops are in range.
- `engine::egress`: a re-emitted announce with an exact buffer always produces
  a well-formed broadcast announce packet, and a one-byte-short buffer rejects
  before claiming a full packet was written.

## Fuzzing

The cargo-fuzz package is isolated under `fuzz/` so it stays out of the normal
workspace build. Use a nightly toolchain:

```sh
cargo +nightly fuzz check
cargo +nightly fuzz run wire_announce_parse -- -max_total_time=30
cargo +nightly fuzz run egress_reemit_round_trip -- -max_total_time=30
```

Current targets:

- `wire_announce_parse`: arbitrary bytes enter the wire parser; any parsed
  header is re-encoded, and announce-shaped payloads are passed through announce
  validation. The corpus includes a real RNS announce vector as a hex seed.
- `egress_reemit_round_trip`: fuzzed hop counts, fanout targets, and output
  slack exercise re-emitted real announces. Serialization must preserve the
  announce payload, produce the expected broadcast header, retain
  engine-computed `fire_on` targets, and reject one-byte-short buffers.

## Mutation Testing

`cargo-mutants` reads `.cargo/mutants.toml` from the source-tree root. The
checked-in config narrows the first lane to contract-heavy surfaces: wire
parsing, RNS serial framing, typed egress, and announce acceptance:

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
