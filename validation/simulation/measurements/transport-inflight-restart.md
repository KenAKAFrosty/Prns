# Transport restart at request and response ingress

macOS arm64, based on `fb3cbadf6`, 2026-09-27.

## Coverage

Four new manual-fleet tests cross two independent choices: left/right forwarding
node and request/response ingress. They reuse the six-node, five-segment bounded
topology from the transport restart slice. Both transports run real production
forwarding; the client and server have no direct medium path.

Each test performs two restart cycles. Before each teardown, a positive control
starts a request, stops at the chosen ingress boundary, then resumes and receives
its complete original payload. The fault case starts a separate request and
stops at the same boundary, but cancels the receiving transport before its next
poll. The boundary detector:

- Inspects only trace events emitted after the new request was admitted.
- Requires a transmission from the exact neighboring endpoint toward that
  transport, with the expected wire context and link address.
- Requires a matching `ReceptionQueued` event for the same transmission ordinal,
  receiving endpoint, original delivery copy and current simulation tick.
- Polls one actor at a time and returns immediately on that send/queue boundary,
  with no intermediate settling or time advancement.
- Rejects trace truncation and bounds the poll count.

This does not guess a packet ordinal or wall-clock sleep. The selected transport
has a real packet queued but has not had another poll to consume it. In the
response cases, the server has already received and handled the request and
emitted the response before teardown.

After cancellation, the pending request remains unresolved through millisecond
49 and returns exactly `SendRequestFailure::Timeout` at millisecond 50. A pair
behind the surviving transport exchanges on its original link during the wait.
Cancellation itself leaves the coordinated clock unchanged and produces no
graceful-stop completion. Actor counts include the outstanding request and then
release its slot at settlement.

The transport is rebuilt with fresh actor/attachment IDs and unchanged logical
interface identities, starts with an empty route table, and explicitly
rediscovers the four leaf destinations. A newly established crossing link
exchanges a complete payload; the unaffected pair still uses its original link.
The second cycle repeats this sequence with the replacement transport. Shared
fixture assertions require all sixteen attachments to detach, no pending actors
or deliveries after final shutdown, no receive drops and no trace truncation.

## Meaning and limits

These are in-flight interruption tests, complementing the earlier requests
issued after a transport was already down. The response cases illustrate an
important application boundary: a timeout does not establish that the remote
operation was never performed. This slice does not add automatic retries,
idempotency tokens or exactly-once application semantics.

Production behavior is unchanged. This is host Tokio simulation over the virtual
frame medium, not a firmware or native network-stack run. Packet matching reads
real wire headers; it does not decrypt or forge traffic. Production crypto and
announcement jitter still use OS entropy. Actor cancellation runs destructors;
physical power loss and persistent-state recovery remain separate coverage.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`:

- `cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet queued_at_ingress --quiet` (four tests).
- `cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet --quiet` (thirteen tests).
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation`.
- `bash validation/hygiene/fmt-docs.sh` and `git diff --check`.

No firmware, Miri/ISA, native networking, hardware smoke or benchmark run is
claimed for this test-only slice.
