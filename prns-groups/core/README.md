# Portable group core

The `prns-groups-core` crate supplies shared IDs, checked revisions, scoped monotonic
operation tickets, capacity accounting, fixed publication metadata, clock admission
and bounded recipient progress. The wider contract covers addressing, authorization,
record admission, synchronization and delivery evidence. It supports
`no_std` with caller-owned fixed-capacity storage. Messaging, telemetry, shared
state and commands consume the same mechanisms through application profiles.

The core owns group/control identity, authorization eras, immutable records and
targets, deadlines, custody/receipt evidence and bounded reconciliation. Security
adapters own keys, encryption audiences and profile-specific epochs or sessions.
Applications own schemas, projections and effects. Transport, time, entropy and
recoverable storage are supplied through bounded platform interfaces.

This directory includes a dependency-free `#![no_std]` Rust crate with no
`alloc` import. `PublicationMetadata` identifies a group, immutable record,
author incarnation, application namespace/schema, original control and fixed
validity window. The application profile supplies the maximum window and allowed
future clock skew. Unknown clocks or intervals overlapping the deadline return
`TimeUncertain`; they establish neither new admission nor expiry.

`DeliveryProgress` borrows caller-owned target storage and rejects empty or
duplicate target sets. A durable receipt must match the exact group, record and
target incarnation. Duplicate receipts are idempotent; removal and expiry do not
count as delivery. Late receipts can add historical evidence without erasing
removal or extending the original deadline. `all_received` requires a receipt
for every original target, including the author if explicitly targeted.

These transitions take authenticated facts, not raw network claims. The caller
verifies signatures and authorization and atomically persists the metadata,
body commitment, original targets and updates. A local storage commit receipt
is distinct from a remote endpoint's durable receipt. The core performs no wire
authentication, MLS processing, persistence, executor scheduling or application effects.
Services must preserve the original metadata/targets when reopening progress;
constructing a new in-memory value is not permission to change a durable record.

The [store coordinator](../store/README.md) and [MLS adapter](../security/mls-rs/README.md)
consume these shared types without adding crypto or allocation to the core.

`enrollment` validates invitation validity intervals and the structural part of
single-member Add and Remove successors. Both bind the exact predecessor and next
epoch, require canonical unique endpoints and preserve remaining incarnations.
Removal must retain the immutable owner. These checks do not authenticate control
metadata or MLS commits; adapters verify those and commit the resulting state
atomically. Group closure is a separate terminal decision, not an empty roster.

`retry` provides the allocation-free `PGR1` request-token encoding and a bounded
retry policy. Callers supply cryptographic entropy and trusted time, authenticate
requests separately, check exact retained results first, and persist the
inclusive retirement floor atomically with deleting retry results. Tokens are
not credentials. Unknown legacy IDs are rejected rather than treated as new work.
`StoreIncarnation` and `OperationTicket` encode a nonzero 24-byte incarnation and
nonzero 64-bit sequence into the existing 32-byte operation ID. Checked `next()`
never wraps. These are local recovery tickets; applications still need separate
network record IDs and group replay policy.

`sync` supplies allocation-free interface/path policy: configured first-hop
bitrate, hop count and observed Link RTT select bounded work, deadlines, idle
spacing and failure backoff. A fast first hop cannot establish fast downstream
capacity. The native carrier and runners consume this policy; an embedded
executor can reuse it with caller-owned timers and buffers. Policy decisions
are estimates, not airtime reservations or delivery evidence.

Read the [architecture](../docs/design.md), [behavioral contract](../docs/behavior.md),
[resource policies](../docs/policies.md), and [embedded support guide](docs/embedded.md).
The [integration boundary](docs/embedded-participants.md) distinguishes reusable
library seams from the complete board-member service that is not implemented.
