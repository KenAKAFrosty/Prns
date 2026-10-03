# Concurrent full-node restart waves and fleet abort

macOS arm64, based on `0c6aef729`, 2026-09-27.

## Scope and bounds

Three new manual-fleet cases use 128 real Tokio nodes on one manual runner and
one frame medium. Sixty-four isolated pairs each have a client and server. They
are concurrent actors, not sixty-four independent runtimes or sequentially
created two-node tests. Production request handling, link establishment, crypto,
timeouts and interface lifecycle remain in use.

The medium admits 128 live endpoints with one neighbor each, eight receive
entries per endpoint, 128 pending-delivery slots and 32,768 trace events. The
runner admits 256 actors. All identities are configured deterministically;
production ephemeral crypto still uses OS entropy. These are growable-heap host
nodes with inline crypto, not fixed-storage Embassy nodes or worker-pool tests.

The fixture boots nodes serially without advancing simulated time, then admits
each traffic round concurrently. The one-neighbor topology prevents unrelated
pairs from becoming alternate routes. Echo payloads carry distinct pair and
round markers and are compared as complete 96-byte values.

## Restart waves

One test restarts servers belonging to even pairs, then odd pairs. The second
reverses that cohort order. Each wave cancels all 32 selected servers before
polling surviving nodes again. Clients and the other 32 servers remain live.

For each wave:

1. Requests on the 32 interrupted links remain unresolved through millisecond
   49 and time out exactly at 50. During that wait, the unaffected 32 pairs
   return their own complete payloads on their original links.
2. All selected servers are rebuilt at the same simulated tick using unchanged
   configured identities and logical interface IDs, but fresh task and endpoint
   IDs. Old task cancellation returns `NotLive` after replacement. The original
   clients are not rebuilt.
3. Explicit rewiring and announcements enable 32 concurrent replacement-link
   establishments. Every replacement link ID differs from its predecessor;
   links in the unaffected cohort remain unchanged.
4. Requests are issued on the obsolete links while all 64 current links exchange
   exact new payloads. Only those 32 obsolete requests time out, again at exactly
   50 milliseconds. Fresh traffic cannot settle an old waiter.
5. All 128 node clocks are observed together. Original nodes retain their elapsed
   time; rebuilt nodes measure elapsed time from their own boot origins on the
   same global clock. No scenario resets the clock to simulate a reboot.

After both waves, virtual time is exactly 200 milliseconds. Every pair exchanges
again and all 128 current nodes shut down normally. All 192 attachments, including
the 64 retired server incarnations, have exactly matching detachments.

## Whole-fleet abort

The third case establishes and exercises all 64 links, isolates every pair, and
starts 64 requests. After they are pending, it drops the entire runner with 192
live actors rather than individually cancelling nodes or requesting shutdown.
The medium must show complete teardown without advancing the clock.

The same driver advances to the abandoned requests' former deadline. A new runner
then boots 128 fresh nodes on that clock and medium. Interface identities remain
stable and endpoint IDs are fresh. Old control handles remain alive across boot;
their shutdown channels are closed, and dropping them cannot stop the new nodes.
Actor IDs are scoped to a runner and are deliberately not compared across this
whole-runner replacement.

All replacement pairs establish fresh links and exchange both immediately and
after another explicit clock advance. Exact per-node elapsed clocks and final
shutdown are checked. All 256 old/new attachments detach, with no retained actor
or pending delivery at the end.

## Capacity and evidence checks

At full occupancy, an extra medium attachment and a cross-pair neighbor edge
are refused with exact typed errors and no trace mutation. Simultaneous clock
observers fill all 256 actor slots; another actor is refused, then all observers
settle and free their slots for subsequent work.

Final borrowed trace inspection verifies every received frame stayed within its
configured pair, all attachment IDs detached exactly once, no receive queue
overflowed, and no trace events were discarded. Historical endpoint identities
are retained only in the bounded final test audit, not added to production state.

## Interpretation and verification

Production behavior and firmware memory are unchanged by this slice. These
tests extend the existing 128-node correctness baseline to concurrent restart
and full-runner abort; they do not establish a maximum fleet size, throughput,
thousands of full nodes, multi-hop convergence, BLE/Wi-Fi stack behavior, physical
power loss or byte-for-byte replay. Rebuild and rediscovery are explicitly driven
by the harness; automatic application retry is not implied.

Host Cargo commands use `CARGO_INCREMENTAL=0`:

- `cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet churn --quiet` (three cases).
- `cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet --quiet` (sixteen cases).
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation`.
- `bash validation/hygiene/fmt-docs.sh` and `git diff --check`.

No firmware, Miri/ISA, native network-stack, hardware or benchmark run is claimed.
