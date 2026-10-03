# Response-size accounting

Status: packet, whole Resource and segmented Resource value accounting corrected,
including metadata-bearing files. Additional boundary and ownership coverage remains.

## Completed packet slice

Packet responses now count exactly the bytes delivered after removing the outer
request-ID envelope. Values are opaque to this layer: bin8/bin16/bin32 headers
count in full, and a nil value counts as one byte. Zero is a real bound; unlimited
remains explicit. Neither wire encoding nor adapter decoding changed.

The owner tests cover exact boundaries, arbitrary values, binary-header-shaped
bytes, empty-to-nil encoding, one terminal result, receipt cleanup, and continued
link use. The mixed-runtime BLE tests reproduce the old discrepancy (Tokio
accepted 257 bytes under a 256-byte limit while Embassy's completion buffer
refused it) and now prove the shared-core refusal on both endpoint pairings.

Adapter audit: Tokio and Embassy retain the encoded value unchanged. The native
host and legacy N-API request wrapper subsequently unwrap complete binary values;
their limit still applies before decoding. N-API's `packed.length` is therefore
the packet budget, not `data.length`. WASM's engine event projection retains the
encoded bytes. Native/C and N-API fixture limits now include binary value headers.

## Completed whole-Resource slice

Whole, metadata-free Resources now enforce the same encoded-value limit as
packets. Advertisement admission discounts at most the fixed outer response
envelope using saturating subtraction; existing transfer/storage ceilings still
apply. Conclusion checks the actual value before any delivery, including legacy
raw bodies with no prefix to discount and bodies resumed after decompression.
Wrong enclosed request IDs settle as `ResponseTransferFailed(TransferCorrupt)`.
Whole uncompressed streams must match their advertised length, as inflated whole
streams already had to do.

Owner tests cover canonical and legacy forms, compression, false lengths,
zero/exact/overflow/unlimited limits, terminal settlement and retired state.
Mixed-runtime BLE scenarios now accept exact 1,200-byte response-value budgets
and fill Embassy's existing 2 KiB completion capacity. They still refuse an
oversized response and reuse the same links afterward. No shipping buffer or
queue capacity changed. See [verification evidence](../../validation/simulation/measurements/whole-resource-response-limits.md).

## Original observation

The mixed Tokio/Embassy Resource capstone found that a 1,200-byte application
response is rejected with `maximum_response_bytes = 1200`, but succeeds when the
limit includes `RESPONSE_WIRE_OVERHEAD`. That original test deliberately named
it an envelope limit; the corrected test now asserts response-value limits.

- [Resource admission](../src/routing/links/resources/receive/gate.rs) previously
  compared all advertised uncompressed `data_bytes` against the request limit.
- [Packet admission](../src/routing/ingress/links.rs) previously subtracted two
  bytes unconditionally. It now uses the complete response value's length.
- [Resource conclusion](../src/routing/links/resources/receive/conclude.rs) strips
  the response envelope, with compatibility for legacy raw-body responses.
  Metadata and segmented responses have additional accounting paths.
- Embassy completion buffers retain application response bytes, not the outer
  request/response envelope. Their capacity is also used as the core limit;
  discounting the outer envelope restores the full usable capacity for whole,
  metadata-free responses.

## Completed whole-file slice

Whole metadata-bearing responses count literal file bytes after removing the
verified metadata block. Advertisement admission cannot know metadata length,
so the final value check runs after verification/inflation; the existing
uncompressed-stream, fixed-transfer and heap-memory ceilings still apply.
Files remain literal even when their leading bytes resemble a response envelope,
including the first segment of a split file. This matches the metadata-bearing
response path in the pinned RNS 1.4.2 and 1.5.0 implementations.

Owner tests cover metadata framing, arbitrary file bytes, exact bounds,
compression, malformed metadata, allocation ceilings and retired state. Mixed
Tokio/Embassy BLE scenarios exercise the shared static-file command, exact file
budgets and Embassy's existing 2 KiB completion buffer. See
[verification evidence](../../validation/simulation/measurements/metadata-resource-response-limits.md).

## Completed split-failure prerequisite

A mismatched request ID in the first split-response envelope now fails the request
as `ResponseTransferFailed(TransferCorrupt)`. Previously that segment was silently
omitted while assembly continued, allowing the tail alone to settle successfully.
Both normal opening and resumed decompression propagate the failure before
advancing the assembly. Failed admitted split transfers also release their assembly state,
including cancellation, malformed metadata, refused hashmaps and exhausted retries.
Whole-transfer failures do not erase a separate split assembly waiting on the link.

Buffered Tokio and Embassy request APIs discard provisional chunks on failure;
tests also prove the next request can reuse the awaiter/slot. Direct journal
consumers must wait for successful final settlement before publishing the value.
No stored fields, capacities, wire formats or response-size admission bounds change.
See [verification evidence](../../validation/simulation/measurements/split-response-failures.md).

## Completed continuation-admission prerequisite

The assembly's stored segment count now constrains every continuation, alongside
its original hash and next index. Completed chains admit no extra segment, and
index comparison cannot wrap at `u64::MAX`. Admission rechecks this same contract
after a queue wait or application decision, before allocating transfer buffers or
claiming a response receipt. A stale queued continuation is removed without
changing the current assembly or settling its request; a still-valid queued
continuation resumes normally. No retained fields, capacities or wire formats change.

See [verification evidence](../../validation/simulation/measurements/split-continuation-admission.md).

## Completed link-ownership prerequisite

Request receipt lookup now requires both the authenticated link and the request
ID. Previously, another link could name a live request and deliver a packet
response, claim its timeout through a Resource offer, or settle it as failed.
All receipt reads, claims, timeout changes and settlements now share this
link-scoped lookup, including delayed Resource completion and pending offers.
The receipt already stores the owning link; no retained fields, capacities or
wire formats change. Public receipt methods now require `&LinkId`; there is no
ID-only compatibility alias. All in-repository callers have been migrated.

Fixed/heap receipt tests cover both lookup-key components, colliding IDs on
separate links, whole policy snapshots and exact settlement. Core-engine tests
use two authenticated links and prove that wrong-link packets, offers and late
cancellations leave the other request able to complete. This is cross-link
isolation, not yet same-link split-chain correlation or cumulative accounting.
See [verification evidence](../../validation/simulation/measurements/response-link-ownership.md).

## Completed split-position ownership prerequisite

Each admitted transfer now retains its offered original chain hash. Opening and
resumed decompression verify that hash, the segment index and total count against
the current assembly before proving or publishing bytes. A superseded transfer
fails as `TransferCorrupt`; its cleanup cannot clear an unrelated or advanced
assembly. Advancement itself also requires the exact named chain position.
The original hash is a required argument to `IncomingResources::accept`, and
`IncomingAssemblies::advance` now requires the original hash, index and count.
There are no permissive link-only compatibility methods for either operation.

This adds a 32-byte identity field to each incoming transfer state, without
changing buffer capacities or the wire. Overlapping-chain tests cover normal
opening, delayed decompression and cancellation, followed by exact completion of
the replacement response. This protects assembly position; the following slice
also binds request correlation within one link. See
[verification evidence](../../validation/simulation/measurements/split-position-ownership.md).

## Completed split-correlation prerequisite

Each assembly now retains its semantic association: unsolicited, request with ID,
or response with ID. Continuations must preserve that complete association at
initial admission, queued promotion, completion and advancement. Initial validation
runs before response policy so a retargeted oversized continuation cannot fail the
unrelated request it names. A stale split transfer also cannot settle the receipt
of a replacement chain answering the same request or change its deadline.

`IncomingAssemblies::begin`, `fit` and `advance` require an explicit
`AssemblyCorrelation`; fixed and heap tables retain it with the owning row. No
wire shape, buffer size or transfer capacity changes. Deterministic core-engine
tests reproduce retargeting and same-request replacement failure with encrypted
frames. The exact failure injections are not yet mixed-runtime simulator scenarios;
passing simulator suites are separate regression evidence. See
[verification evidence](../../validation/simulation/measurements/split-correlation-ownership.md).

## Remaining segmented scope

The mixed-runtime simulator now exercises successful three-segment static-file
responses, zero-budget refusal and reuse through both runtimes with a test-only
512-byte transfer window. Raw journals and buffered request futures are checked
separately. These initial scenarios did not enforce exact cumulative value budgets or
inject inconsistent peers; see
[simulation evidence](../../validation/simulation/measurements/segmented-resource-simulation.md).

The [interruption scenarios](../../validation/simulation/measurements/interrupted-segmented-resource-simulation.md)
now isolate virtual BLE after the first verified segment in both directions.
Raw requests settle once as `Timeout`, with no further response data, and the
same nodes complete new transfers after rediscovery and old-link retirement.
The [buffered-request cases](../../validation/simulation/measurements/buffered-resource-interruption.md)
also hold the next outgoing Resource advertisement and isolate the radios.
Calibrated raw journals confirm the first segment precedes this cut; the real
Tokio/Embassy buffered APIs return `Timeout` without partial success and recover
after old-link retirement. Both bounded Embassy request slots are reusable
concurrently.

The [live-link expiry scenarios](../../validation/simulation/measurements/live-link-response-expiry.md)
drop continuation advertisements without blocking unrelated frames or retiring
the link. They reproduced a retained assembly after request timeout: a subsequent
request on a different link could not use the receiver's single assembly slot.
Shared-core receipt expiry now clears only the response assembly with the same
link and request ID. Both runtimes then complete a new three-segment response
on the other link and reuse the original links. No persistent storage column or
capacity changes; expired receipt results carry the already-stored packet hash.

The [receipt-pressure scenarios](../../validation/simulation/measurements/receipt-pressure-response-recovery.md)
also reproduce between-segment assembly retention when a newer request displaces
the old receipt. Culling now uses the same link/request-scoped assembly cleanup
as expiry. A one-receipt simulator profile proves that the replacement request
still completes, and another link can subsequently use the single assembly slot.

Extend the delivered-value byte-counting contract to segmented Resource forms.
The [cumulative stream-size prerequisite](../../validation/simulation/measurements/split-stream-size-validation.md)
now checks verified stream bytes against each admitted segment's size declaration
before proving or delivering that segment. It rejects cumulative overflow or
overrun, and requires exact equality at the final segment, using the existing
stream counter. The [size-identity slice](../../validation/simulation/measurements/split-stream-size-identity.md)
now retains the first size declaration and requires it at admission, queued
promotion/expiry, completion, advancement and failure cleanup. The distinct
delivered-value budget was completed in the following slices.
The [value-accounting slice](../../validation/simulation/measurements/split-value-accounting.md)
adds separate verified-value progress and cumulative checks before chunk delivery.
That slice retained conservative admission: a trial relaxation exposed missing
completion-time cancellation in the simulator's sender-reuse checks. The
[completion-cancellation slice](../../validation/simulation/measurements/split-response-budget-cancellation.md)
now requires fresh entropy on each completion API and cancels oversized split
responses without proving the offending segment. Admission defers the value budget
to verified cumulative bytes, retaining independent stream and allocation bounds.
Owner tests use real request limits from creation, and the mixed-runtime capstone
proves zero-budget refusal, refusal after a provisional segment, exact 1,200-byte
completion and sender/receiver reuse on the same links.
The [atomic progress follow-up](../../validation/simulation/measurements/atomic-stream-progress.md)
also makes advancement enforce that same checked stream total before mutating
the assembly, rather than trusting callers and using saturating addition.
Separate early allocation protection from final delivered-payload validation;
loosening an advertisement check alone would admit oversized legacy raw bodies.
Keep the authoritative accounting in shared core, with host completion buffers
enforcing their own actual storage capacity rather than compensating for wire
headers independently.

Cover exact limits, one-byte overflow, zero and unlimited bounds, raw bytes,
MessagePack bin8/bin16/bin32, canonical and legacy Resource envelopes, metadata,
compression, and multi-segment responses. No prefix may be silently subtracted
from arbitrary application data. Check overflow-safe advertised bounds and
bounded allocation before receiving an untrusted transfer.

Extend owner tests and the mixed-runtime capstones with exact Embassy completion
capacity boundaries. The
[segmented completion-capacity scenarios](../../validation/simulation/measurements/segmented-completion-capacity.md)
now cover 2,047, 2,048 and 2,049-byte files under a 2,048-byte budget with five
segments, raw journals and real buffered request APIs in both directions. They
verify late refusal without partial success, one terminal result, same-link
recovery, and reuse of both embedded completion slots. Further inconsistent-peer
injections and whole/split overlap arbitration remain separate work.
Preserve Remote Control's fixed response bounds and stock-Reticulum wire
interoperability. Audit reusable native/Node.js/WASM client semantics before
publishing a changed limit contract; do not introduce a transport-specific fix.

Request expiry and receipt culling between segments are covered. The
[active-transfer cases](../../validation/simulation/measurements/active-response-culling.md)
also cull a request after continuation admission while its next data part is held,
then release that late part without disturbing the replacement request. Raw
calibration and buffered APIs are tested separately; this is not exhaustive
coverage of every possible transfer phase. The
[refusal cleanup slice](../../validation/simulation/measurements/refused-continuation-cleanup.md)
now reclaims a failed response assembly when a continuation exceeds the response
limit, cannot fit transfer storage, encounters a full offer queue, or expires
while queued. Stale queued continuations are discarded before they can fail a
replacement chain. These new refusal cases have core-engine reproductions, not
yet mixed-runtime simulator injections. Segment counts and request correlation are now stable across the
chain; validate advertised stream totals before loosening admission. Arbitration
between a whole Resource response and an overlapping split response naming the same
request remains open. The
[competing-packet slice](../../validation/simulation/measurements/competing-packet-response.md)
now prevents a response packet from publishing or settling a request already owned
by a matching admitted split assembly. Its simulator reproduction pauses continuation
advertisements after the first verified segment, injects the competing packet, then
proves exact original completion and same-link recovery in both runtime directions.
The [sender-exclusion scenarios](../../validation/simulation/measurements/whole-response-sender-exclusion.md)
also establish that the normal responder API refuses a competing whole Resource
locally as `LinkBusy`. With explicit spare test storage, a separate link completes
a whole response while the original split waits, then the original completes and
both runtimes recover. This verifies sender exclusion, not rejection of a whole
offer emitted by an inconsistent peer. The
[competing-whole-offer slice](../../validation/simulation/measurements/competing-whole-offer.md)
now cancels fresh and queued whole offers when a matching split assembly owns the
request, before policy refusal or queue expiry can settle it. These are deterministic
core-engine reproductions, not yet mixed-runtime inconsistent-peer injections.
Whole transfers admitted before the split owner appeared remain a separate
completion-time arbitration gap. The
[superseded-whole failure slice](../../validation/simulation/measurements/superseded-whole-failure.md)
now preserves split ownership when such a whole transfer fails, is cancelled, or
exhausts retries. A real-admission core-engine fixture proves the split response
still completes. The
[superseded-whole completion slice](../../validation/simulation/measurements/superseded-whole-completion.md)
also cancels verified whole completion and delayed decompression results before
proof or publication while the split owns the request. The
[worker-verdict follow-up](../../validation/simulation/measurements/superseded-whole-worker.md)
explicitly covers delayed whole-open verdicts (opened, pre-digested and unavailable
fallback) and stale replay before and after transfer-slot reuse. Its streamed-open
follow-up holds a real first-span job across split admission and checks completion
both between segments and during continuation reception. The
[detached-buffer follow-up](../../validation/simulation/measurements/superseded-whole-detached.md)
also exercises growable-heap transfer ownership through the tail worker, including
refusal of duplicate buffer take/dispatch. The
[ended-claim follow-up](../../validation/simulation/measurements/whole-response-ended-claim.md)
also rejects late whole completion after split success or request expiry, rather
than publishing it as an unsolicited Resource. Its direct and decompression
cases are core-engine fixtures. The
[ended-claim whole-worker follow-up](../../validation/simulation/measurements/whole-response-ended-worker.md)
also checks opened, pre-digested and unavailable-worker verdicts after success or
expiry, including stale replay. The
[ended-claim streamed-worker follow-up](../../validation/simulation/measurements/whole-response-ended-stream.md)
covers first-span return after success/expiry and copied or detached tail return
after success. The
[in-flight watchdog follow-up](../../validation/simulation/measurements/whole-response-ended-watchdog.md)
drives real transfer deadlines while a copied or detached tail is held: the
continuation exhausts retries, the worker times out, and late returns remain inert.
Mixed-runtime inconsistent-peer injections remain separate coverage work.

The [blocked-send runtime follow-up](../../validation/simulation/measurements/blocked-send-link-wakes.md)
holds a real continuation send until cancellation, then proves stale-link teardown
and recovery on ESP32/Apple and nRF52/BlueZ runtime pairings. It exposed missing
receipt/channel/pairing wake updates from shared-core link-deadline teardown.
This is not independent conflicting-response injection.

The [Tokio late-continuation follow-up](../../validation/simulation/measurements/tokio-late-continuation.md)
reverses those runtime roles. Tokio retains the held send beyond Embassy's request
timeout, so releasing it proves a real late advertisement is harmless and both
request slots and original links remain reusable. Independent conflicting-peer
injection remains unimplemented.

The [channel-wake teardown follow-up](../../validation/simulation/measurements/link-teardown-channel-wakes.md)
directly exercises an outstanding channel send and request through stale-link
expiry, checks cached versus recomputed schedules, and rejects duplicate
settlements at their former deadlines. This is shared-core fixture coverage of
the earlier simulator-discovered wake issue, not a new mixed-runtime scenario.

The [pairing-wake follow-up](../../validation/simulation/measurements/link-teardown-pairing-wakes.md)
adds a live controller awaiting-offer attempt to stale-link expiry. Matching-link
teardown clears it; unrelated-link teardown preserves its complete state and
deadline. This is direct engine-fixture assurance for the third wake family,
not coverage of a full pairing exchange or its persistence phases.

The [active-continuation competition follow-up](../../validation/simulation/measurements/competing-packet-active-continuation.md)
returns to real mixed-runtime traffic: after a continuation data part is observed
being dropped, a competing response packet cannot replace or settle the original
split request. Both directions and compatibility profile pairs retain exact
completion and same-link recovery. Independent whole-Resource peers remain open.

The [buffered-competition matrix](../../validation/simulation/measurements/buffered-response-competition.md)
now exercises the normal Tokio and Embassy buffered APIs before the first segment,
between segments and during continuation reception. Each preserves exact file
bytes, concurrent same-link progress and subsequent request-slot reuse. Disabling
the existing shared guard makes both adapters complete prematurely in all three
phases; the guard remains unchanged in this assurance-only slice.

The [retired-response matrix](../../validation/simulation/measurements/retired-buffered-response.md)
follows successful or timed-out buffered requests with a fresh segmented request
on the same link. Newly emitted packets naming the retired ID cannot settle or
contaminate the replacement across three reception phases, both runtimes and
both profile pairs. Exact receipt identity matching is unchanged and is directly
challenged by a targeted diagnostic mutation.

The [abandoned-waiter follow-up](../../validation/simulation/measurements/abandoned-request-waiters.md)
separates dropping a local buffered future from retiring its protocol receipt.
Late completion cannot contaminate a fresh segmented request or prevent reuse of
both Embassy completion slots. It explicitly checks Tokio's private consumption
and Embassy's unawaited application delivery rather than assuming cancellation
semantics are identical across adapters.

The [partial-response cancellation matrix](../../validation/simulation/measurements/abandoned-segmented-responses.md)
then drops those callers after one or two verified segments. Successful orphan
completion exposes only Embassy's unconsumed suffix; timeout emits no extra
segments. Both adapters permit concurrent waiter reuse and later spare-link
assembly reuse. A targeted timeout-cleanup omission breaks both runtime tests.
Tokio's private receiver timeout is bounded by an observation window, not
misrepresented as the responder's independently observed settlement instant.
