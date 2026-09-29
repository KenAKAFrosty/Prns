# Buffered requests retain split-response ownership across receive phases

macOS arm64, based on `a80cb074e`, 2026-09-26.

## Motivation

The original [competing-packet finding](competing-packet-response.md) reproduced
premature response publication through raw journals. Its discussion of different
Tokio and Embassy buffering consequences came from adapter inspection. This slice
directly exercises both normal buffered request APIs under that competition.

## Scenario matrix

Six independently named tests cover three phases for each requester runtime;
each runs ESP32/CoreBluetooth and nRF52/BlueZ profiles, for twelve combinations:

- Before the first verified segment: drop Resource parts from the first part.
- Between segments: forward the first advertisement, then drop continuations.
- During continuation reception: forward the first segment's two parts, then
  drop subsequent Resource parts while advertisements and requests flow.

Every injection waits for an actual dropped frame. For part loss, a real Resource
request must already have elicited data from the sender. No arbitrary sleep is
used to infer phase. An opt-in responder-side probe captures the matching
link/request identity from the runtime's request event. The responder then issues
a competing packet through its ordinary command API; no engine access, key
extraction or unauthenticated wire fabrication is involved.

A second buffered echo request completes on the same link while the original
Resource remains blocked. The original future must still be pending. Loss then
ends, and the original must return exactly all 1,200 file bytes with the measured
RTT. Tokio uses an explicit 1,200-byte response budget; Embassy uses its normal
2,048-byte bounded response buffer. The fixture verifies that buffered requests
leave no raw response events or request settlements behind, and checks all three
embedded responder settlements where Embassy is the responder.

Both Embassy request slots are reused concurrently afterward. Existing recovery
checks then exercise refusal, raw segmented delivery and buffered delivery in
both directions on the same links. Fixture teardown rejects leftover observations,
wire gates, unexpected link closures and radio leaks.

## Bounded observation support

`RequestProbe` is confined to the test binary. It retains at most one matching
request's link, path and request ID, never its body; unarmed and unrelated traffic
retain nothing. Arming over an existing watch or overwriting an unconsumed
observation fails explicitly. Paused-time tests cover scoped observation, wakeup,
observation before waiting, reuse, cancellation of the waiting future, and both
capacity refusals. No production storage or runtime event API changed.

## Diagnostic evidence and limits

Disabling only the shared packet-response split-ownership guard made all six
phase/runtime tests fail because the buffered request completed before loss was
released. Each failing test stops on its first profile pair; this diagnostic does
not claim an exhaustive mutant run over both profiles. The guard was restored
before final verification. The unmodified implementation passes all twelve
combinations.

This proves premature completion is rejected through both real buffered APIs.
The diagnostic deliberately stops at that safety assertion, so it does not
measure the exact corrupted value each unguarded adapter would have returned.
Independent whole-Resource competitors, worker-pool replay, physical RF/stacks
and every possible overlap phase remain separate work. Production behavior,
wire format, capacity and firmware layout are unchanged.

## Verification

Passed on macOS arm64, with `CARGO_INCREMENTAL=0` for tests and clippy:

- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble buffered_competition --quiet` (six scenarios, two profiles each).
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble --quiet` (70 tests, including four request-probe unit tests).
- `python3 validation/run.py run --suite virtual-device-simulation`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `bash validation/hygiene/fmt-docs.sh` and `git diff --check`.

No firmware matrix, Miri/ISA, physical hardware, stock interoperability or
benchmark rerun is claimed for this test-only slice.
