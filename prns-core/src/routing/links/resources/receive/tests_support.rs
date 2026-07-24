use crate::crypto::{
    x25519_diffie_hellman, Ed25519PublicKey, Ed25519SecretKey, X25519PublicKey, X25519SecretKey,
};
use crate::engine::test_support::{filled_frame, routable_descriptor, TestStorageLayout};
use crate::engine::IngestIo;
use crate::engine::Journaled;
use crate::engine::{CommandId, SetResourceStrategy, Settlement};
use crate::engine::{Directive, EngineReaction, EngineState, InstantMillis};
use crate::engine::{EngineCommand, IssuedCommand};
use crate::identity::IdentityHash;
use crate::interfaces::AttachedInterfaces;
use crate::interfaces::{InboundPacket, InterfaceId};
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::{ResourceBody, ResourceMetadata, ResourceSend};
use crate::routing::links::resources::{ResourceFailureCause, ResourceHash, ResourceStrategy};
use crate::routing::links::table::LinkActivation;
use crate::routing::links::table::{InitiatedLink, RespondingLink};
use crate::routing::links::{LinkId, LinkKey};
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::storage::StorageLayout;
use crate::wire::{DestinationHash, BROADCAST_MTU};

pub(crate) fn bytes_from_hex(s: &str) -> std::vec::Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
        .collect()
}

pub(crate) const LINK_ID: &str = "000102030405060708090a0b0c0d0e0f";
pub(crate) const RESPONDER_DESTINATION: DestinationHash = DestinationHash::new([0x77; 16]);
pub(crate) const INITIATOR_SCALAR: &str =
    "3333333333333333333333333333333333333333333333333333333333333333";
pub(crate) const RESPONDER_PUBLIC: &str =
    "ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b";
pub(crate) const CASE1_BZ2: &str = "425a6839314159265359cf3017f4000207918040000e6f9e002000902980000a54a7a869ea794d3227c13a1382644e09a09a1342684f213f04c09b1382704ec2684d89e04c8ab61302604d09d09d89fc5dc914e142433cc05fd0";

/// umsgpack.packb({"name": "case.bin", "flag": 7}): the block the reference-driven metadata fixtures carry.
pub(crate) const META_PACKED: &str = "82a46e616d65a8636173652e62696ea4666c616707";

/// bz2.compress(3-byte-BE(21) ‖ packed ‖ case1 plaintext): the whole 1384-byte composite compressed, exactly what the reference feeds bz2.
pub(crate) const META_CASE1_BZ2: &str = "425a6839314159265359c5bada7900000071d04080020040013fef9e00100004403000b8450000064c82800003264052a5008da684f227a37e3ae33ea278137546a26f89e7fb3cbe7a13509a89a09fbcc4e2132f9a7f84e027613d6627bd44d8274fcef13b09c04e3547bc09a09cf026e130277132136136b79ff177245385090c5bada790";

/// The reference's in-stream framing: `struct.pack(">I", len)[1:] ‖ packed`.
pub(crate) fn metadata_block(packed: &[u8]) -> std::vec::Vec<u8> {
    let prefix = (packed.len() as u32).to_be_bytes();
    let mut block = std::vec::Vec::with_capacity(3 + packed.len());
    block.extend_from_slice(&prefix[1..]);
    block.extend_from_slice(packed);
    block
}

pub(crate) fn link_id() -> LinkId {
    LinkId::new(bytes_from_hex(LINK_ID).try_into().unwrap())
}

pub(crate) fn link_key() -> LinkKey {
    let scalar: [u8; 32] = bytes_from_hex(INITIATOR_SCALAR).try_into().unwrap();
    let public: [u8; 32] = bytes_from_hex(RESPONDER_PUBLIC).try_into().unwrap();
    let shared = x25519_diffie_hellman(&X25519SecretKey::new(scalar), &X25519PublicKey(public));
    LinkKey::derive(&link_id(), &shared)
}

pub(crate) fn lane() -> InterfaceId {
    InterfaceId::new([0xEE; 8])
}

pub(crate) fn engine_with_active_link() -> EngineState<TestStorageLayout> {
    active_engine::<TestStorageLayout>()
}

pub(crate) fn engine_with_responding_link() -> EngineState<TestStorageLayout> {
    let mut engine = EngineState::<TestStorageLayout>::default();
    engine
        .links
        .track_responding(RespondingLink {
            link_id: link_id(),
            key: link_key(),
            requested_at: InstantMillis(500),
            timeout_at: InstantMillis(5_000),
            mtu: BROADCAST_MTU,
            initiator_signing: Ed25519PublicKey([0x99; 32]),
            destination: RESPONDER_DESTINATION,
            identity: IdentityHash::new([0x77; 16]),
            proof_strategy: ProofStrategy::ProveNone,
        })
        .unwrap();
    engine
        .links
        .activate_responding(
            &link_id(),
            crate::units::RttMillis::new(250),
            lane(),
            InstantMillis(1_000),
        )
        .unwrap();
    engine
}

pub(crate) fn active_engine<S: StorageLayout>() -> EngineState<S> {
    let mut engine = EngineState::<S>::default();
    engine
        .links
        .track_initiated(InitiatedLink {
            link_id: link_id(),
            destination: RESPONDER_DESTINATION,
            initiator_secret: X25519SecretKey::new([0x33; 32]),
            link_signing: Ed25519SecretKey::new([0x33; 32]),
            requested_at: InstantMillis(500),
            timeout_at: InstantMillis(5_000),
            command_id: CommandId(1),
        })
        .unwrap();
    engine
        .links
        .activate_initiated(
            &link_id(),
            link_key(),
            &LinkActivation {
                rtt: crate::units::RttMillis::new(250),
                mtu: BROADCAST_MTU,
                attached_interface: lane(),
                peer_signing: Ed25519PublicKey([0x99; 32]),
            },
            InstantMillis(1_000),
        )
        .unwrap();
    engine
}

pub(crate) fn advertisement_frame(data: &[u8], candidate: Option<&[u8]>) -> std::vec::Vec<u8> {
    let mut sender = engine_with_active_link();
    advertise_from(&mut sender, data, candidate)
}

pub(crate) fn advertise_from<S: StorageLayout>(
    sender: &mut EngineState<S>,
    data: &[u8],
    candidate: Option<&[u8]>,
) -> std::vec::Vec<u8> {
    let mut frame = None;
    sender.ingest_send_resource_into(
        &ResourceSend {
            id: CommandId(7),
            link_id: link_id(),
            body: ResourceBody {
                data,
                compressed_candidate: candidate,
                metadata: ResourceMetadata::None,
            },
            correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
        },
        InstantMillis(1_500),
        &mut |bytes: &mut [u8]| bytes.fill(0xA5),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                frame = filled_frame(fill);
            }
        },
    );
    frame.expect("the sender advertises")
}

pub(crate) struct InboundCapture {
    pub(crate) frames: std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)>,
    pub(crate) settlements: std::vec::Vec<(CommandId, Settlement)>,
    pub(crate) received: std::vec::Vec<(ResourceHash, std::vec::Vec<u8>)>,
    pub(crate) received_metadata: std::vec::Vec<(ResourceHash, std::vec::Vec<u8>)>,
    pub(crate) segment_metadata: std::vec::Vec<(ResourceHash, u64, std::vec::Vec<u8>)>,
    pub(crate) failed: std::vec::Vec<(ResourceHash, ResourceFailureCause)>,
    pub(crate) segments: std::vec::Vec<(ResourceHash, u64, std::vec::Vec<u8>)>,
    pub(crate) response_segments: std::vec::Vec<(CommandId, RequestId, u64, std::vec::Vec<u8>)>,
    pub(crate) assembled: std::vec::Vec<(ResourceHash, u64)>,
    pub(crate) mismatched: std::vec::Vec<(InterfaceId, InterfaceId)>,
    pub(crate) requests: std::vec::Vec<(RequestId, std::vec::Vec<u8>)>,
}

pub(crate) fn feed<S: StorageLayout>(
    engine: &mut EngineState<S>,
    frame: &[u8],
    at: u64,
) -> InboundCapture {
    feed_on(engine, frame, lane(), at)
}

pub(crate) fn feed_judged<S: StorageLayout>(
    engine: &mut EngineState<S>,
    frame: &[u8],
    at: u64,
    should_accept_resource: &mut impl FnMut(&crate::routing::links::resources::ResourceOffer) -> bool,
) -> InboundCapture {
    feed_inner(engine, frame, lane(), at, should_accept_resource)
}

pub(crate) fn feed_on<S: StorageLayout>(
    engine: &mut EngineState<S>,
    frame: &[u8],
    source_interface: InterfaceId,
    at: u64,
) -> InboundCapture {
    feed_inner(
        engine,
        frame,
        source_interface,
        at,
        &mut |_: &crate::routing::links::resources::ResourceOffer| false,
    )
}

fn feed_inner<S: StorageLayout>(
    engine: &mut EngineState<S>,
    frame: &[u8],
    source_interface: InterfaceId,
    at: u64,
    should_accept_resource: &mut impl FnMut(&crate::routing::links::resources::ResourceOffer) -> bool,
) -> InboundCapture {
    let mut capture = InboundCapture {
        frames: std::vec::Vec::new(),
        settlements: std::vec::Vec::new(),
        received: std::vec::Vec::new(),
        received_metadata: std::vec::Vec::new(),
        segment_metadata: std::vec::Vec::new(),
        failed: std::vec::Vec::new(),
        segments: std::vec::Vec::new(),
        response_segments: std::vec::Vec::new(),
        assembled: std::vec::Vec::new(),
        mismatched: std::vec::Vec::new(),
        requests: std::vec::Vec::new(),
    };
    let mut raw = frame.to_vec();
    engine.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(at),
            source_interface,
            bytes: &mut raw,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&[routable_descriptor(source_interface)]),
            now: InstantMillis(at),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource,
            sink: &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        capture.frames.push((target, frame));
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    capture.settlements.push((id, settlement));
                }
                EngineReaction::Journaled(Journaled::ResourceReceived {
                    hash,
                    metadata,
                    data,
                    ..
                }) => {
                    capture.received.push((hash, data.to_vec()));
                    if let Some(metadata) = metadata {
                        capture.received_metadata.push((hash, metadata.to_vec()));
                    }
                }
                EngineReaction::Journaled(Journaled::ResourceFailed { hash, cause, .. }) => {
                    capture.failed.push((hash, cause));
                }
                EngineReaction::Journaled(Journaled::ResourceSegmentReceived {
                    original_hash,
                    segment_index,
                    metadata,
                    data,
                    ..
                }) => {
                    capture
                        .segments
                        .push((original_hash, segment_index, data.to_vec()));
                    if let Some(metadata) = metadata {
                        capture.segment_metadata.push((
                            original_hash,
                            segment_index,
                            metadata.to_vec(),
                        ));
                    }
                }
                EngineReaction::Journaled(Journaled::ResponseSegmentReceived {
                    command_id,
                    request_id,
                    segment_index,
                    data,
                    ..
                }) => {
                    capture.response_segments.push((
                        command_id,
                        request_id,
                        segment_index,
                        data.to_vec(),
                    ));
                }
                EngineReaction::Journaled(Journaled::ResourceAssembled {
                    original_hash,
                    total_size_bytes,
                    ..
                }) => {
                    capture.assembled.push((original_hash, total_size_bytes));
                }
                EngineReaction::Journaled(Journaled::LinkInterfaceMismatch {
                    attached_interface,
                    arrived_on,
                    ..
                }) => {
                    capture.mismatched.push((attached_interface, arrived_on));
                }
                EngineReaction::Journaled(Journaled::RequestReceived {
                    request_id, data, ..
                }) => {
                    capture.requests.push((request_id, data.to_vec()));
                }
                _ => {}
            },
        },
    );
    capture
}

/// Book the pending row a sent request would have left, returning the request id its response must name.
pub(crate) fn track_pending_request<S: StorageLayout>(
    engine: &mut EngineState<S>,
    command_id: CommandId,
    sent_at: u64,
    timeout_at: u64,
) -> RequestId {
    use crate::identity::IdentitySigningPublicKey;
    use crate::routing::dedup::PacketHash;
    use crate::routing::delivery::receipts::{OutstandingReceipt, ReceiptKind};
    use crate::wire::{DestinationType, PacketType, WireContext};
    let packet_hash = PacketHash::of_fields(
        DestinationType::Link,
        PacketType::Data,
        &link_id().to_address(),
        WireContext::Request,
        &b"the request we sent"[..],
    );
    let request_id = RequestId::of_packet(&packet_hash);
    engine.receipts.track(OutstandingReceipt {
        packet_hash,
        command_id,
        kind: ReceiptKind::SendRequest,
        peer_signing_key: IdentitySigningPublicKey::new(Ed25519PublicKey([0x99; 32])),
        sent_at: InstantMillis(sent_at),
        timeout_at: InstantMillis(timeout_at),
    });
    request_id
}

pub(crate) fn advertise_response_segment_from<S: StorageLayout>(
    sender: &mut EngineState<S>,
    id: CommandId,
    request_id: RequestId,
    data: &[u8],
    candidate: Option<&[u8]>,
    segment: crate::routing::links::resources::ResourceSegment,
    at: u64,
) -> std::vec::Vec<u8> {
    let mut frame = None;
    sender.ingest_send_resource_segment_into(
        &ResourceSend {
            id,
            link_id: link_id(),
            body: ResourceBody {
                data,
                compressed_candidate: candidate,
                metadata: ResourceMetadata::None,
            },
            correlation: crate::routing::links::resources::ResourceCorrelation::Response(
                request_id,
            ),
        },
        segment,
        InstantMillis(at),
        &mut |bytes: &mut [u8]| bytes.fill(0xA5),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                frame = filled_frame(fill);
            }
        },
    );
    frame.expect("the responder advertises its response segment")
}

pub(crate) fn accept_everything<S: StorageLayout>(engine: &mut EngineState<S>) {
    set_strategy(
        engine,
        ResourceStrategy::Accept {
            max_uncompressed_bytes: 1 << 20,
            accept_compressed: true,
        },
    );
}

pub(crate) fn set_strategy<S: StorageLayout>(
    engine: &mut EngineState<S>,
    strategy: ResourceStrategy,
) {
    let mut settled = std::vec::Vec::new();
    engine.ingest_command_into(
        IssuedCommand {
            id: CommandId(9),
            command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                link_id: link_id(),
                strategy,
            }),
        },
        AttachedInterfaces::new(&[routable_descriptor(lane())]),
        InstantMillis(1_500),
        &mut |bytes: &mut [u8]| bytes.fill(0xB1),
        &mut |reaction| {
            if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                reaction
            {
                settled.push((id, settlement));
            }
        },
    );
    assert!(matches!(
        settled[0],
        (CommandId(9), Settlement::SetResourceStrategy(Ok(()))),
    ));
}

pub(crate) fn four_part_payload() -> std::vec::Vec<u8> {
    b"resource parts ride raw on the wire! ".repeat(41)
}
