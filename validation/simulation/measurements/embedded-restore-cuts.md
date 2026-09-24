# Embedded persistence restore across torn updates

The journal power-loss fixture now passes surviving flash images through
`EmbeddedFlashPersistence::restore`, using fresh production engine and Remote
Control assembly state for every boot. Snapshot decoding, logical-time recovery,
restore diagnostics and publication of interface group settings belong to the
production Embassy persistence owner, not a simulator reimplementation.

## Evidence

A valid baseline stores independent group sets for two stable interface IDs and
a persisted timebase. An update changes the first interface from one group to
two while preserving the second interface's distinct group. All 89 write-prefix
and verification-read boundary cuts through that append are exercised.

Each cut reconstructs the owner twice from the identical surviving byte image.
Before the final commit word completes, both boots must publish the complete
confirmed configuration; afterward, both must publish the complete candidate.
The independent oracle compares the entire restore report and typed snapshot,
including zero unrelated restore counters and the exact persisted clock floor.
Diagnostics must contain exactly the corresponding restore report, and the
owner must not report unsaved state.

The same sequential campaign then appends a malformed group snapshot inside a
valid committed journal record. The owner must refuse that payload, count the
refusal, and retain the previous valid configuration. Finally, a boot from blank
flash must publish an empty configuration and use raw boot time, rather than
retain the prior device image's published groups or clock floor.

## Per-node ownership

`DiscoveryGroupConfigurationStoreExchange` now owns one node's restored snapshot
and bounded request/completion mailbox. `with_discovery_group_store` accepts an
explicit exchange, including an ordinary borrowed local value. The default
constructor and board-wide helper functions retain the existing global exchange;
their selector is zero-sized, not a new per-board pointer. The node facade accepts
either owner, and pairing persistence asks its own wrapped owner about pending
group changes instead of consulting a process-global queue.

The power-cut campaign uses isolated exchanges. A second integration test restores
two devices with identical interface IDs but different groups, in both boot orders.
Rebooting one with blank flash must not clear the other's published configuration.
An Embassy owner test additionally queues changes on two separate owners and
engines: write failure, capacity refusal, completion and caller cancellation stay
local to the corresponding exchange. These tests use the production write path,
not a second implementation of mailbox or persistence behavior.

## Scope

Production impact: explicit discovery-group persistence ownership is now available.
Default single-board behavior and persisted bytes are unchanged. No dependency or
wire-format changes are needed; the default selector has a zero-size assertion.

The writes under fault still use the production journal API directly. This
does not exercise the persistence owner's queued write transaction, command
activation/rollback, radio discovery startup, grant restoration or a complete
running node reboot. It also is not evidence for Tokio's separate store path.

This isolates discovery-group persistence, not every Embassy service or a fleet
of complete running nodes. Consumers of the board-global helper API still share
that default exchange; multi-node callers must use each node's explicit exchange.

The original bounded NOR model and its limitations still apply: contiguous
byte-prefix tears, immediate I/O, no flash physics or ISA emulation.

## Verification

Cargo commands use `CARGO_INCREMENTAL=0` on macOS arm64:

```console
cargo test --locked -p prns-simulation --test journal_power_loss -- --nocapture
cargo clippy --locked -p prns-simulation --all-targets --features controlled-time,heap-profile -- -D warnings
python3 validation/run.py run --suite virtual-device-simulation
cargo test --workspace --locked --quiet
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

The original restore slice passed five journal/restore tests, retaining the 694
compaction cuts alongside the 89 owner-restore cuts. Its registered simulation
suite passed in 62.535 seconds. The ownership follow-up adds a sixth journal test
and a separate two-owner Embassy mailbox/write regression.

The ownership follow-up passed all six journal tests, all 141 Embassy library
tests, the registered simulation suite (86.479 seconds), workspace tests,
simulation and Embassy clippy, formatting/docs, registry and website checks.
The Embassy library also passed a no-std `thumbv7em-none-eabihf` target check.
This is not a full firmware-link, resource-budget or hardware qualification run.
