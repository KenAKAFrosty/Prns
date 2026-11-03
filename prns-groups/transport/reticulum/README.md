# Reticulum group synchronization

`prns-groups-reticulum` connects the group service to the application's existing
`HostClient`. It uses an identified Reticulum Link and bounded request/response
exchanges. Larger responses travel as ordinary Reticulum Resources. It creates
no node, identity, executor or background worker and has no host shutdown authority.
The adapter is a native `std`/Tokio implementation; the group core remains
allocation-free and the service remains `no_std + alloc`.

With `default-features = false`, the crate keeps its canonical peer mapping,
destination derivation, cursor, limits, and storage contract available without the
native host or Tokio. The default `network` feature supplies the Link operations,
ingress and synchronization functions described below. These are the same types
used by local managed enrollment; there is no separate offline mirror.

The optional `contact-card` feature adds offline enrollment claim selection.
Use `contact_card::enrollment_service_name_hash()` when signing a card through
`prns-contact-card`; after URI/token decoding and signature verification,
`contact_card::enrollment_recipient(&card)` accepts only this service's claim.
The result is the identity for an ordinary Invite or enrollment exchange after
consumer consent. A card for `lxmf.delivery` or the group's sync destination is
not an enrollment claim. No LXMF, live host, directory or announcement is needed
to verify/select the recipient; `enrollment_destination_for` derives its route
address offline. Live path discovery and identified Links still authenticate the
exchange. The card does not grant membership or imply network reachability.

## Composition

For independent local identities on a shared host, use `Network::scoped` and
`enrollment::Network::scoped` with a `HostServiceScope` instead of inheriting the
host's default identity. The caller registers matching service destinations and
supplies the same immutable identity to its group store. Scoped connections
identify with that identity and retain operation/Link ownership across canceled
waiters. Fence new work, drain admitted operations and complete the node's owned
retirement barrier when pausing. Existing constructors below remain supported.

1. Add `destination_config(limits)` to the existing host's configuration. This
   creates a `prns.groups` SINGLE destination under its existing identity, with
   an initially empty requester allow list. Announces contain no group IDs.
2. Create `Ingress` before starting the host and forward application events to
   `on_event` or the borrowed `on_event_ref`. It filters destination/path, requires an identified requester,
   checks framing/size and reserves a bounded queue slot before copying. The
   application owns draining the returned receiver and reporting overflow.
3. Construct `Network` with the borrowed host, local destination and explicitly
   authenticated `Peer` mappings, then call `authorize_peers`. Each mapping binds
   an endpoint incarnation to its Reticulum identity and destination. These are
   enrollment facts, not inferred from MLS endpoint bytes or discovery.
4. Serve queued jobs with `serve_mut(network, service, job, options)` or
   `serve_storage_with_options` for a shared storage lane. Supply the receiver's
   trusted clock and store incarnation; neither comes from a request. Only
   configured readers can inspect inventory, fetch, or submit ciphertext. A
   forwarding member need not be the original MLS sender. The immutable `serve`
   and original `serve_storage` remain compatible pull-only handlers.
5. Call `reconcile` with a peer, trusted clock, local store incarnation and work
   budget. It pulls a frozen inventory in two passes: records, then receipts.
   Accepted records and pending receipt work commit before any cursor advances.
   Visiting an already accepted record also repairs a receipt intent left by a
   prior interruption. Only authenticated MLS receipts change delivery evidence.
6. After durable publication, call `push`/`push_storage` with the exact retained
   envelope's `EnvelopeInfo`. It opens one identified Link, explicitly discovers
   support, submits that envelope, and processes an optional authenticated MLS
   receipt in the response. It creates no background scheduler. Schedule polling
   in both directions for repair and retain completed checkpoints. A complete
   inventory prefix describes examined history, not delivery or future arrivals.

`SyncReport.cursor` resumes a partial suffix or retains a completed high-water
mark. Use `report.has_more()` for scheduling, not `cursor.is_some()`. Completion
does not clear the cursor. Save it with `SyncCursor::to_bytes`
and restore with `from_bytes`; it is a versioned local checkpoint, not a signed
network claim. Reusing it for another peer, group or local endpoint fails.
A completed checkpoint first requests the current snapshot. If unchanged, that
one request finishes the poll without inventory pages or fetches. If its head
grew, both the record and receipt passes start above the old completed head.
Newer arrivals beyond the frozen head wait for the next snapshot. A changed
generation or regressed head starts from zero. If generation changes during a
partial scan, the peer returns `RestartInventory`; discard the cursor and retry.
Expiry maintenance changes that generation and leaves gaps in monotonic inventory
positions. Page limits bound the number of live entries scanned/returned, not the
distance between cursors. Validation still requires ordered positions within the
page's frozen head and its declared next cursor.
The fixed-size local `PGC` checkpoint is version two (173 bytes), including
the common lower bound for both passes. Unsupported checkpoint versions are
rejected. The checkpoint is local storage; snapshot/inventory/fetch wire frames
use version one. Bind persisted coverage to enrollment and store incarnation;
the native runner does this in its storage adapter. A checkpoint cannot replace
the corresponding accepted records/receipts or be restored independently of them.
Lost cursors or results are safe to rescan because accepted envelopes and exact
receipt output are durable and deduplicated. Link loss or cancellation never
confirms device receipt. A connection closes its owned link on completion/drop,
including a successful late connection after its caller has stopped waiting.
The host's bounded protocol-operation lane owns path discovery, Link establishment
and requests. Responses enqueue directly through the existing node API, so a
request waiting for its response cannot block a reciprocal incoming response
behind the host's serialized command lane. Path/key lookup and identification
have inner timeouts; an admitted Link establishment remains owned until the core
settles it, so a late successful Link can still be closed safely. The caller's
exchange timeout bounds its wait, not that final settlement lifetime.

The caller serializes service access, including ticket allocation and receipt
emission. Network callbacks do no storage work. `SyncStorage`, `serve_storage` and
`reconcile_storage` split storage calls from network waits so a shared database
lane can serve incoming work during an outgoing pull. The original direct
`GroupService` helpers remain available for caller-driven tests.
The native runner supplies scheduling and backoff
without a shared service lock across reciprocal pulls.

## Prompt delivery and same-Link receipts

Push is an optional group-carrier extension; Reticulum packets, Links, Resources,
MLS ciphertext and receipt semantics are unchanged. Capability discovery combines
its result with the current snapshot, so an initial pull can fit discovery,
inventory, Fetch and receipt Submit into four requests on a constrained path.
Two-request callers use legacy pull to preserve continuation progress. A partial
scan negotiates on its new Link; capability knowledge is not a durable claim.

A Submit carries group, exact envelope ID, ciphertext length/bytes and a requested
receipt reply allowance. The receiver validates the complete frame, authenticated
reader, group, size and ciphertext hash before its ordinary MLS receive path.
It commits acceptance and repairs/retains its local receipt before replying. A
replayed Submit returns that same retained receipt; it does not encrypt a fresh
retry. A lost response or process restart leaves the record/receipt recoverable
from the existing durable transactions and inventory.

A push reply may include at most 2048 bytes of retained MLS receipt ciphertext,
also bounded by both endpoints' hard byte/frame allowances. Larger or unavailable
receipts remain available through repair polling. The sender accepts delivery
only after the service validates that receipt; the inline result must match the
submitted record and the receiving endpoint incarnation. The carrier's plain
acceptance/rejection bit is never a device receipt. Submitting a receipt returns
no nested receipt, avoiding acknowledgement loops.

A pull returns its newly retained receipt with Submit on the same authenticated
Link when request and total ciphertext budgets permit. Known records also repair
and resubmit the exact receipt when revisited. Exhaustion leaves the receipt in
inventory. No extra Link, network-wide announce, or discovery identity is created
for the response. `SyncReport.bytes` includes this reverse receipt traffic;
`fetched` continues to count fetched envelopes only.

An explicit old-peer `InvalidRequest`, or expiry of the read-only capability
probe (including the core request timer), reports unsupported and permits legacy
pull. A silent legacy group dispatcher may incur one bounded probe timeout.
Malformed capability responses, authentication failures and other transport
errors are not acceptance. The native runner temporarily caches unsupported
observations per enrolled peer/configuration; `Network::with_push_enabled(false)`
suppresses probes during that cooldown. Polling remains the recovery path when
push is unsupported, offline, interrupted, or budget limited. Submit has 77 bytes of
carrier overhead versus Fetch's 41; an envelope that only fits Fetch must use
polling recovery. Treat that per-record size limit as pull-only, not peer failure
or permission to re-encrypt/split its MLS ciphertext.

The new independent-host tests verify one Link for capability+Submit, four
requests on one Link for snapshot/inventory/Fetch/receipt, duplicate suppression,
actual reply loss after durable commit followed by database reopen and exact
retry, old peers that reject or silently drop capability requests, hard byte
preflight and malformed/group/hash/unauthorized rejection. They use actual
Reticulum Links and Resources over a controlled carrier, not radio hardware.

## Interface-aware synchronization

`Network::path_capabilities` reads Reticulum's selected route and joins its exact
physical interface ID to the runtime's timing inventory. This uses configured
nominal bitrate, not recent traffic counters or the name of an attached radio.
Each connection adds the measured end-to-end Link RTT and updates it after a
response. A fast BLE first hop through a router cannot establish fast downstream
capacity: unknown and multi-hop paths use the conservative policy estimate.

The allocation-free `prns_groups_core::sync::SyncPolicy` selects inventory page size,
request/envelope work per pass, and bounded operation deadlines from those hints.
`with_sync_policy` overrides the defaults. `Limits.exchange_timeout` remains the
local host-command timeout and a minimum network budget, subject to the policy's
maximum; it does not bound the entire synchronization pass. Request
waits preserve Reticulum's adaptive Link/Resource timers. An additional bounded
application deadline covers estimated serialization, round trips and transfer
overhead. It is an estimate, not a throughput or airtime guarantee.

Snapshot and inventory replies have their own byte ceilings; fetches use the
inventory-advertised envelope size for both response admission and time budgeting.
A soft per-pass byte target yields a continuation after one larger envelope when
it still fits the caller's hard frame/byte bounds. It cannot starve valid larger
application profiles by repeatedly rejecting them at the soft target. The report
includes the measured path so caller scheduling can back off without establishing
a fresh Link just to inspect its route. Cancellation retains the existing scoped
Link cleanup, including late establishment after the caller stops waiting.

Completed checkpoints avoid rescanning unchanged retained history. An initial
scan or a generation reset still consumes bounded passes, and the configured
idle interval is not an upper bound on new-message latency. Loss of a checkpoint
safely repeats deduplicated work. Arbitrary eviction of live accepted records
would require a separate repair/reset policy; the current authenticated-expiry
maintenance does not require restoring expired content.

Destination announcements are separate from synchronization. The native
multi-group runner announces two fixed service destinations at startup and every
six hours plus jitter, regardless of group count. Cold identified connections
use ordinary path requests; they need not wait for the periodic refresh. See
the [announce strategy and reference comparison](docs/announce-strategy.md).

## Enrollment carrier

`enrollment::destination_config` adds a separate `prns.groups.enrollment` SINGLE
destination to the same host, with `/prns/groups/enrollment/v1` requests. Its
node policy is `AllowAll` because a prospective member has no group peer binding
yet. `enrollment::Ingress` rejects unidentified requests before application frame
allocation and has its own bounded queue. The underlying node may still receive
a bounded request Resource before the application rejects it; this is not a
network-wide admission or denial-of-service defense.

`enrollment::Network::exchange(identity, frame)` derives that identity's enrollment
destination, checks the verified announce key against the expected identity,
establishes and identifies a Link, and returns one bounded response. The complete
application frame, including its own header, is at most 131072 bytes. Caller
timeouts and cancellation close owned Links without stopping the host. Responses
are independently enqueued, permitting reciprocal exchanges.
Enrollment uses the same bounded capability-aware carrier policy;
`exchange_with_capabilities` also returns its measured path for scheduling.

`RequestJob::requester()` is the Link-authenticated identity. An invitation handler
must bind authorization to it rather than an identity claimed by the payload,
validate its own wire format and replay rules, and commit durably before replying.
The carrier itself neither enrolls members nor stores or authorizes invitations.
Its request jobs deliberately omit payload bytes from debug output. Responding
through the wrong local destination or request path fails.

For several groups on the sync destination, `request_group(frame)` validates the
complete fixed sync request syntax and extracts its group without allocation.
This is routing only: the selected service still checks requester membership and
its own page and work limits. It accepts no partial header or trailing bytes.

## Bounds and carrier format

All limits are explicit: peer count, queue slots, frame bytes, inventory page
scan size and request timeout. A complete packed request must fit a single
Reticulum Resource. Unsolicited Resources stay refused. The request payload uses
Reticulum's MessagePack binary framing; responses use the existing node's binary
response encoder. No Reticulum packet, routing or Link protocol changes are made.

The experimental carrier uses `/prns/groups/v1/sync`, a `PGT` version-1 prefix
and big-endian integers. Snapshot, paged inventory and exact fetch operations remain unchanged. Two
optional opcodes add capability discovery with snapshot (4) and exact-envelope
Submit (5). Inventory hints carry envelope ID, record dependency, kind,
position and byte length. The receiver checks framing, cursor monotonicity,
size and ciphertext hash, then MLS verifies the author and content. Known
ciphertext is checked against authoritative local metadata before repairing its
receipt intent. Unknown versions, trailing bytes and malformed lengths fail.
The format is not yet a frozen public interoperability specification.

A call requires at least two requests of budget so an inventory page and fetch
can make progress. `max_envelopes` counts fetched envelopes; `max_bytes` counts transferred MLS
ciphertext in both directions, including submitted/returned receipts, excluding
inventory, request and Reticulum overhead.
Request count and frame ceilings separately bound control traffic. An envelope
larger than the entire byte budget returns `EnvelopeBudget` with its required
size and resumable cursor. Queue overflow causes timeout/retry without acceptance.
Configure transport page size no larger than the service's envelope limit.

The service retains exact own and accepted incoming envelopes in the same atomic
transaction as MLS/application state. Thus another member can supply content and
receipts while the author is offline. Count and total retained-byte quotas are
independent of body/history limits. Exporting locally generated ciphertext still
checks the original MLS output's authorization/retirement fence.

## Evidence and remaining work

The [integration test](tests/independent_endpoints.rs) runs four actual independent
Reticulum hosts over a controlled frame carrier. Three have separate MLS
identities and redb databases; the fourth is unauthorized. It verifies:

- Chat and 4 KiB telemetry through authenticated Links and Resource transfers.
- Offline catch-up through another member while the original author is offline.
- Receipt reconciliation to all original delivery targets, distinct from transfer
  proofs and availability advertisements.
- Bounded continuation across a database close/reopen, repeated synchronization,
  insufficient budgets, malformed requests and unauthorized-peer rejection.
- An expired record rejected before restart, its dependent receipt rejected
  afterward, and later valid records/receipts completing the same inventory scan.
- Scoped link closure without stopping the shared host.

The test bridge moves complete Reticulum frames; it never hands publication bytes
directly to a receiving group service. This is protocol integration evidence,
not a TCP/radio hardware, Python-router or mobile-lifecycle qualification.

The [enrollment transport tests](tests/enrollment.rs) also use independent hosts.
They cover identity metadata despite spoofed payload claims, an unknown-to-group
requester, exact 128 KiB responses, request and response Resource transfers,
reciprocal calls, rejection of unidentified requests, pre-network oversize refusal
and cancellation without stopping the host. These test carrier authentication
and bounds, not invitation authorization, MLS enrollment or replay durability.

Synchronization requires explicitly enrolled peer mappings. The native runner
handles invitation/control exchanges separately and reconstructs each group's
sync network from its current authenticated configuration. Third-party blind custodians and protected mobile storage remain work. The service
provides bounded expiry maintenance for inventory, history and exact outputs.
Record admission reserves logical service envelope/byte/change and MLS output
capacity for each target's first receipt; independent backend limits still apply.
Missing dependencies and uncertain time remain retryable errors. An authenticated
expired record leaves a rejection tombstone. Receipts carry their
original deadline, verified against live records; an authenticated expired unknown
receipt can be rejected after its dependency has been reclaimed. Neither route
invents delivery evidence. Exact rejection deduplication lasts only as long as its
retained operation fact; pruned ciphertext replay does not become fresh content.
See the [service format and retention contract](../../service/README.md) for
receipt deadline validation and bounded dependency reclamation.

```sh
cargo test --locked --manifest-path prns-groups/Cargo.toml -p prns-groups-reticulum
cargo test --locked --manifest-path prns-groups/Cargo.toml -p prns-groups-service
```
