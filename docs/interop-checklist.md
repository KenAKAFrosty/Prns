# Stock-RNS interoperability test checklist

This checklist records the interoperability behaviors Prns aims to exercise
against stock RNS and offers a starting point for a reusable community test
harness. It is not a definition of Reticulum conformance or a claim about what
every implementation must support. The tests focus on externally observable
behavior rather than internal APIs or implementation structure. When an
operation is meaningful in both directions, the proof should exercise both
directions.

For Prns, a checked item has registered black-box or live evidence against stock
RNS 1.4.2. An unchecked item marks a gap in Prns's registered evidence, not
necessarily missing Prns behavior and never a verdict on another implementation.
Internal unit tests or implementation support alone do not check an item. Prns
currently has registered evidence for 19 of these 28 operations.

## Identity and destinations

- [x] **Identity compatibility**
  - Load the same identity in both binaries, then confirm matching hashes,
    cross-compatible signatures, and cross-compatible encryption.
- [x] **SINGLE announcements**
  - Have each side announce a SINGLE destination and confirm the other side addresses
    it successfully.
- [ ] **Announce application data**
  - Send exact application bytes in announcements both ways and confirm each receiver
    reports them unchanged.
- [ ] **PLAIN destinations**
  - Exchange exact PLAIN payloads both ways without an identity or shared key.
- [ ] **GROUP destinations**
  - Configure the same group key and exchange exact GROUP payloads both ways.
- [x] **Ratchets**
  - Require ratchets and prove packets across two distinct announced ratchet
    generations.

## Packets and links

- [x] **Proven SINGLE packets**
  - Exchange exact payloads both ways and confirm valid delivery proofs.
- [x] **Link establishment**
  - Initiate a Link from each implementation and exchange traffic only after both peers
    report it active.
- [ ] **Link packets**
  - Exchange exact direct Link packets both ways and confirm their delivery proofs.
- [x] **Link identification**
  - Have each initiator identify itself and confirm the responder observes and
    authorizes the exact identity.
- [ ] **Link closure**
  - Have each side close a Link and confirm its peer observes a clean remote closure.
- [x] **Packet-backed requests**
  - Send a small named-path request from each side and confirm the exact response.
- [x] **Resource-backed responses**
  - Return an oversized response in both directions and confirm exact completion.
- [x] **Request authorization**
  - Confirm an allowed identity succeeds while an unknown identity receives no
    protected response.

## Resources and streams

- [x] **Resource transfer and metadata**
  - Transfer an exact single-segment Resource with metadata in both directions.
- [ ] **Resource compression**
  - Transfer compressible Resources both ways and confirm compressed transport plus
    exact reconstructed bytes.
- [x] **Multi-segment Resources**
  - Cross the stock segment boundary both ways and confirm multiple completed
    segments plus exact bytes.
- [x] **Resource cancellation**
  - Cancel an active transfer, confirm no partial publication, then complete a fresh
    transfer.
- [ ] **Resource rejection**
  - Refuse an offered Resource and confirm the sender sees rejection with no payload
    publication.
- [x] **Channel messages**
  - Exchange multiple typed messages both ways and confirm exact order and
    acknowledgements.
- [ ] **Buffer streams**
  - Exchange exact bytes across different write and read boundaries and confirm clean
    EOF both ways.

## Routing and transport

- [x] **Path discovery**
  - Discover an initially unknown destination through a transport, report its hops,
    and reach it.
- [x] **Mixed multi-hop forwarding**
  - Exchange exact payloads between stock endpoints through stock and candidate
    transports in series.
- [ ] **Transport tunnel recovery**
  - Reconnect with the same transport identity and confirm the restored route works
    without a fresh endpoint announcement.

## Common adapters

- [x] **TCP client and server**
  - Run the candidate in both TCP roles against stock RNS and exchange proven packets.
- [x] **UDP**
  - Configure complementary endpoints and exchange exact proven payloads both ways.
- [x] **Shared-instance client and server**
  - Run both shared-instance roles against stock RNS and carry valid application
    traffic each way.
- [x] **IFAC authentication**
  - Confirm matching credentials exchange traffic while missing or incorrect
    credentials are rejected.

## Current scope and evidence

The checklist currently includes common TCP, UDP, shared-instance, and IFAC
adapter behavior because those boundaries are useful to Prns and to a potential
shared harness. Hardware-specific adapters, operator utilities, configuration
syntax, and SDK API shapes sit outside its current scope. That boundary is a
testing choice, not a judgment about any implementation's completeness,
validity, or conformance.

Stock RNS utilities may drive a test or provide observations, but evidence for
this checklist should center on the observable interoperation described above.
Its tests aim to treat both implementations as opaque processes and assert only
their inputs, outputs, and observable state.

For Prns, [`validation/manifest.toml`](../validation/manifest.toml) is the
authoritative inventory of registered suites. The [validation
guide](validation.md) explains how to run them and collect reproducible evidence;
this page is a human-readable audit, not another configuration source.
