# Validation Lanes

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

## Kani Proofs

Kani harnesses are gated behind `cfg(kani)`, so normal builds do not compile
them. Run the focused proofs with:

```sh
cargo kani -p personal-rns --harness hops_above_pathfinder_m_always_reject_before_any_other_gate
cargo kani -p personal-rns --harness local_destination_rejects_when_hops_are_in_range
```

Current proof coverage:

- `announce::acceptance`: max-hop rejection wins before later gates, and local
  destinations reject once hops are in range.

## Fuzzing

The cargo-fuzz package is isolated under `fuzz/` so it stays out of the normal
workspace build. Use a nightly toolchain:

```sh
cargo +nightly fuzz check
cargo +nightly fuzz run wire_announce_parse -- -max_total_time=30
```

Current target:

- `wire_announce_parse`: arbitrary bytes enter the wire parser; any parsed
  header is re-encoded, and announce-shaped payloads are passed through announce
  validation. The corpus includes a real RNS announce vector as a hex seed.

## Mutation Testing

`cargo-mutants` reads `.cargo/mutants.toml` from the source-tree root. The
checked-in config narrows the first lane to wire parsing and announce acceptance:

```sh
cargo mutants --list-files
cargo mutants
```

Treat survivors as review prompts, not automatic failures, until the team has
triaged enough runs to decide which mutants are equivalent and which are true
coverage gaps.
