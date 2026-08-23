# Stock-RNS interoperability test checklist

This checklist records the interoperability behaviors Prns aims to exercise
against stock RNS and offers a starting point for a reusable community test
harness. It is not a definition of Reticulum conformance or a claim about what
every implementation must support. The tests focus on externally observable
behavior rather than internal APIs or implementation structure. When an
operation is meaningful in both directions, the proof should exercise both
directions.

For Prns, a checked item has registered black-box or live evidence against the
stock RNS release pinned by
[`validation/manifest.toml`](../validation/manifest.toml). An unchecked item
marks a gap in Prns's registered evidence, not necessarily missing Prns behavior. Internal unit tests or implementation support alone do not check an item. Prns currently has registered evidence for 31 of these 31 operations. Each linked `[x]` opens the executable case providing the primary evidence for that check; one suite may substantiate several observable operations.

## Identity and destinations

- [\[x\]](../validation/interop/cases/rnid_local_interop_smoke.py) **Identity compatibility**
  - Load the same identity in both binaries, then confirm matching hashes,
    cross-compatible signatures, and cross-compatible encryption.
- [\[x\]](../validation/interop/cases/tcp_interop_smoke.py) **SINGLE announcements**
  - Have each side announce a SINGLE destination and confirm the other side addresses
    it successfully.
- [\[x\]](../validation/interop/cases/announce_app_data_interop_smoke.py) **Announce application data**
  - Send exact application bytes in announcements both ways and confirm each receiver
    reports them unchanged.
- [\[x\]](../validation/interop/cases/plain_group_destinations_interop_smoke.py) **PLAIN destinations**
  - Exchange exact PLAIN payloads both ways without an identity or shared key.
- [\[x\]](../validation/interop/cases/plain_group_destinations_interop_smoke.py) **GROUP destinations**
  - Configure the same group key and exchange exact GROUP payloads both ways.
- [\[x\]](../validation/interop/cases/ratchet_interop_smoke.py) **Ratchets**
  - Require ratchets and prove packets across two distinct announced ratchet
    generations.

## Packets and links

- [\[x\]](../validation/interop/cases/udp_interop_smoke.py) **Proven SINGLE packets**
  - Exchange exact payloads both ways and confirm valid delivery proofs.
- [\[x\]](../validation/interop/cases/transport_single_interop_smoke.py) **Transported proven SINGLE packets**
  - Exchange exact proven SINGLE packets between stock endpoints through stock and
    candidate transports in series.
- [\[x\]](../validation/interop/cases/link_packet_interop_smoke.py) **Link establishment**
  - Initiate a Link from each implementation and exchange traffic only after both peers
    report it active.
- [\[x\]](../validation/interop/cases/link_packet_interop_smoke.py) **Link packets**
  - Initiate a Link from each implementation, send an exact direct Link packet to
    its responder, and confirm delivery plus the responder's proof.
- [\[x\]](../validation/interop/cases/rncp_interop_smoke.py) **Link identification**
  - Have each initiator identify itself and confirm the responder observes and
    authorizes the exact identity.
- [\[x\]](../validation/interop/cases/link_closure_interop_smoke.py) **Link closure**
  - Have each side close a Link and confirm its peer observes a clean remote closure.
- [\[x\]](../validation/interop/cases/large_request_interop_smoke.py) **Packet-backed requests**
  - Send a small named-path request from each side and confirm the exact response.
- [\[x\]](../validation/interop/cases/large_request_interop_smoke.py) **Resource-backed responses**
  - Return an oversized response in both directions and confirm exact completion.
- [\[x\]](../validation/interop/cases/remote_management_interop_smoke.py) **Request authorization**
  - Confirm an allowed identity succeeds while an unknown identity receives no
    protected response.

## Resources and streams

- [\[x\]](../validation/interop/cases/rncp_interop_smoke.py) **Resource transfer and metadata**
  - Transfer an exact single-segment Resource with metadata in both directions.
- [\[x\]](../validation/interop/cases/rncp_interop_smoke.py) **Resource compression**
  - Transfer compressible Resources both ways and confirm compressed transport plus
    exact reconstructed bytes.
- [\[x\]](../validation/interop/cases/rncp_interop_smoke.py) **Multi-segment Resources**
  - Cross the stock segment boundary both ways and confirm multiple completed
    segments plus exact bytes.
- [\[x\]](../validation/interop/cases/rncp_interop_smoke.py) **Resource cancellation**
  - Cancel an active stock-to-candidate transfer and interrupt an active
    candidate-to-stock transfer, confirm no partial publication, then complete fresh
    transfers both ways.
- [\[x\]](../validation/interop/cases/resource_rejection_interop_smoke.py) **Resource rejection**
  - Refuse an offered Resource and confirm the sender sees rejection with no payload
    publication.
- [\[x\]](../validation/interop/cases/channel_interop_smoke.py) **Channel messages**
  - Exchange multiple typed messages both ways and confirm exact order and
    acknowledgements.
- [\[x\]](../validation/interop/cases/buffer_stream_interop_smoke.py) **Buffer streams**
  - Exchange exact bytes across different write and read boundaries and confirm clean
    EOF both ways.

## Routing and transport

- [\[x\]](../validation/interop/cases/mixed_multihop_interop_smoke.py) **Path discovery**
  - Discover an initially unknown destination through a transport, report its hops,
    and reach it.
- [\[x\]](../validation/interop/cases/cold_path_request_interop_smoke.py) **On-demand path requests**
  - Begin with only the destination hash, discover it through stock and candidate
    transports, then deliver a proven packet.
- [\[x\]](../validation/interop/cases/mixed_multihop_interop_smoke.py) **Mixed multi-hop forwarding**
  - Exchange exact payloads between stock endpoints through stock and candidate
    transports in series.
- [\[x\]](../validation/interop/cases/route_replacement_interop_smoke.py) **Competing route replacement**
  - Present longer and shorter live routes to one destination and confirm traffic
    follows the accepted replacement.
- [\[x\]](../validation/interop/cases/tunnel_recovery_interop_smoke.py) **Transport tunnel recovery**
  - Reconnect with the same transport identity and confirm the restored route works
    without a fresh endpoint announcement.

## Common adapters

- [\[x\]](../validation/interop/cases/tcp_interop_smoke.py) **TCP client and server**
  - Run the candidate in both TCP roles against stock RNS and exchange proven packets.
- [\[x\]](../validation/interop/cases/udp_interop_smoke.py) **UDP**
  - Configure complementary endpoints and exchange exact proven payloads both ways.
- [\[x\]](../validation/interop/cases/local_interop_smoke.py) **Shared-instance client and server**
  - Run both shared-instance roles against stock RNS and carry valid application
    traffic each way.
- [\[x\]](../validation/interop/cases/ifac_tcp_interop_smoke.py) **IFAC authentication**
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
authoritative inventory of registered suites. From the repository root, first
check that the required development tools, including the Rust toolchain, are
available:

```console
./tools/prns doctor getting-started
```

Then run all interop suites supported by the current host with:

```console
python3 validation/run.py run --domain interop --platform current
```

To run one focused suite, list the available suite IDs and then select one:

```console
python3 validation/run.py list --domain interop --platform current

python3 validation/run.py run --suite interop-route-replacement
```

> NOTE: The validation manifest is the source of truth for which suites the runner
executes. This page is a human-readable short reference, not another configuration source.
