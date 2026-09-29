# Competing packets during a segmented response

Local macOS arm64 candidate based on `f4bd6deae`, 2026-09-26.

## Finding and shared-core correction

A new deterministic simulator scenario reproduced a response packet publishing
and settling a request after its first verified Resource segment. The original
split assembly was still live. This is a simulator-discovered failure, not merely
a synthetic table setup or a speculative ownership concern.

Adapter inspection explains why that event sequence is unsafe: Embassy appends
both whole and segmented response events to its completion buffer, so it could
return a mixture as successful. Tokio replaces the accumulated bytes on a whole
response event, so it could instead discard the original partial response and
return the competing value. The new scenario directly observes raw journals;
these buffered outcomes are inferred from the adapters, not separately injected
through their buffered APIs in this slice.

Shared packet ingress now checks the existing split assembly's link/request
correlation after authenticating and parsing the packet, before response-size
policy or settlement. A matching assembly owns that response; the competing
packet is ignored as superseded. This also prevents an oversized competing packet
from failing the original request. Other requests and other links are unaffected.
The ownership check applies from assembly admission, including before the first
verified segment. No new retained fields, capacities, wire formats or adapter
workarounds are introduced.

## Coverage

The simulator drops continuation advertisements after the first segment while
keeping the link alive and other frames flowing. It issues the competing response
through the real responder command API, verifies no extra requester event, restores
advertisements and checks all three original chunks plus exactly one successful
settlement. The existing recovery sequence then reuses the same links. Both
request directions run for ESP32/CoreBluetooth and nRF52/BlueZ compatibility
pairings over virtual radios, not physical stacks or RF.

The pre-fix focused run failed at the no-extra-event assertion. With the shared
guard it passes. Core owner tests additionally cover matching versus unrelated
request/link ownership and competing packets within or beyond the request limit;
the unrelated assembly remains intact. Those owner tests install assembly rows
directly, independently of the end-to-end simulator reproduction.

A focused manual mutation broadened the guard to any response assembly on the
link, discarding the request-ID comparison. It compiled and failed the owner test
because an unrelated request was incorrectly suppressed. The mutation was restored
and the focused tests rerun. This is diagnostic evidence, not a mutation score.

Whole Resource competition, replacement split chains and every possible overlap
phase remain separate work. This is specifically packet-versus-admitted-split
arbitration, not a claim that all response ownership cases are closed.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`; firmware uses the canonical builder
environment. Passing commands for this slice:

- `cargo test --locked -p prns-core --lib an_admitted_split_response --quiet`.
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble packet_responses_cannot_replace --quiet`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo check --locked -p prns-core --no-default-features`.
- `bash validation/hygiene/fmt-docs.sh` and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- Registered suites: `virtual-device-simulation`, `bluetooth-auto-embassy`,
  `embedded-miri-quick`, `embedded-isa-thumbv7em`, `embedded-isa-riscv32imac`,
  `embedded-isa-xtensa-esp32s3`. Existing Miri/ISA scenarios provide regression
  evidence, not direct embedded execution of the new competition test.
  The simulator run includes 55 passing mixed-runtime BLE tests.
- `./tools/prns build embedded resources report --target t114`: static RAM
  142,996 bytes, RAM headroom 360 bytes, estimated stack remaining 10,352 bytes,
  all unchanged. Image size 623,844 bytes (+64), flash headroom 142,108 bytes.

The readiness doctor reports 0.4 GiB free against the full matrix's 24 GiB
requirement and missing Renode. Full board matrix, platform pilots, physical
hardware, fresh interoperability and performance measurements were not run.
`./tools/prns build embedded resources contracts --check` still reports the
pre-existing stale `tools/release/flasher_memory_contracts.py`, left unchanged.
