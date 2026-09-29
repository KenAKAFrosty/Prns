# Caller cancellation during a fragmented BLE response

macOS arm64, based on `643b8ae52`, 2026-09-27.

## Scope

Dropping the awaiting request future is not a transport failure or a guarantee
of protocol cancellation. This matrix cancels only the manually scheduled
caller actor, leaving all three production Tokio nodes and their original
frame/BLE interfaces running.

Four tests combine two response boundaries (two fragments queued or consumed)
with two replacement timings (before or after the abandoned reply drains).
Each covers both request directions, both CoreBluetooth/BlueZ profile
assignments and cyclic ordering plus version-one seeds 0, 7 and u64::MAX.
That is 64 configurations, each repeated twice. Profiles run over virtual GATT,
not native OS Bluetooth stacks.

The existing fragment observer requires the exact response header/link and
new-send baseline, no completed frame, no counter saturation, the expected
last-polled actor and no time movement. These are real 256-byte echoed replies,
not injected replacement response packets.

## Assertions

- Cancellation changes four registered actors to three, returns `Cancelled`
  exactly once and `NotLive` on repetition. The clock, BLE data snapshot and
  discovery trace are unchanged by dropping the caller. One BLE connection
  remains active.
- Before-drain cases admit two distinct replacement requests on the original
  link before polling any node again. Their exact completion map must contain
  only their own payloads, never bytes from the abandoned request.
- After-drain cases first settle the original response with no registered
  caller completion. Its direction records exactly one additional completed
  send and reassembled frame, with no additional send started. Then both
  replacement requests complete correctly.
- Replacement exchanges finish without advancing time. The independent
  frame-local link remains usable.
- Both media/runtime clocks advance together to the abandoned request's
  explicit 50-millisecond response deadline. No registered actor completion
  occurs, and the retired caller remains `NotLive`.
- Two more distinct requests then reuse the same logical link concurrently.
  Node actor IDs, routes/ingress interfaces, elapsed clocks and connection count
  remain correct. Final shutdown verifies complete detach, no pending frame
  deliveries/connections and untruncated, non-overflowed traces.

All payload markers differ across old/replacement requests and cycles, so a
stale response cannot pass the equality check by coincidence. The actor budget
remains eight; the replacement round uses three nodes and two callers.

## Production impact and limits

Test-only change. No shipping code, simulator-library allocation or cancellation
policy changes. The existing fragment tests now own a child cancellation module
instead of growing one flat file.

The matrix observes registered caller completions and transport progress. It
does not add a raw application-journal observer, establish immediate removal of
Tokio's private completion entry, or prove bounded retention for permanently
unresponsive peers. It is not an exactly-once application guarantee: the
responder has already generated its reply before cancellation.

No native Bluetooth, hardware, firmware build, Miri/ISA, performance benchmark
or exhaustive scheduling claim is made.

## Verification

Passed the focused four-test cancellation matrix, simulation all-target clippy
with warnings denied, and the registered simulation suite (32.525 seconds).
Root/workspace tests, repository formatting/docs checks and `git diff --check`
also passed.
Host Cargo runs used `CARGO_INCREMENTAL=0`; existing ignored tests remain ignored.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet cancelled_ --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
```
