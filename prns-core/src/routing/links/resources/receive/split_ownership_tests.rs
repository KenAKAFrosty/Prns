use super::tests_support::*;
use crate::crypto::Ed25519PublicKey;
use crate::engine::test_support::TestStorageLayout;
use crate::engine::{
    CommandId, DeliveryEvidence, EngineState, InstantMillis, PacketReceiptDelivered,
    ResourceDecompressionCompleted, SendRequestFailure, Settlement,
};
use crate::identity::IdentitySigningPublicKey;
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::receipts::{OutstandingReceipt, ReceiptKind};
use crate::routing::links::data::write_link_packet;
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::assembly::{AssemblyCorrelation, SegmentFit};
use crate::routing::links::resources::{ResourceFailureCause, ResourceHash, ResourceSegment};
use crate::units::{ByteLimit, RttMillis};
use crate::wire::{WireContext, BROADCAST_MTU};

enum Opening {
    Inline,
    Inflated,
}

struct FirstSegment {
    sender: EngineState<TestStorageLayout>,
    command: CommandId,
    request: RequestId,
    hash: ResourceHash,
    body: [u8; 128],
    part: std::vec::Vec<u8>,
}

impl FirstSegment {
    fn admit(
        receiver: &mut EngineState<TestStorageLayout>,
        byte: u8,
        opening: Opening,
        at: u64,
    ) -> Self {
        let command = CommandId(u64::from(byte));
        let packet_hash = PacketHash::new([byte; 32]);
        let request = RequestId::of_packet(&packet_hash);
        assert_eq!(
            receiver.receipts.track(OutstandingReceipt {
                packet_hash,
                command_id: command,
                kind: ReceiptKind::request(
                    link_id(),
                    ByteLimit::Unlimited,
                    crate::engine::SendRequestIntent::Application
                ),
                peer_signing_key: IdentitySigningPublicKey::new(Ed25519PublicKey([0x99; 32])),
                sent_at: InstantMillis(1_800),
                timeout_at: InstantMillis(20_000),
            }),
            None
        );
        Self::admit_response(receiver, command, request, byte, opening, at)
    }

    fn admit_response(
        receiver: &mut EngineState<TestStorageLayout>,
        command: CommandId,
        request: RequestId,
        byte: u8,
        opening: Opening,
        at: u64,
    ) -> Self {
        let mut sender = engine_with_active_link();
        let body = [byte; 128];
        let candidate = match opening {
            Opening::Inline => None,
            // Exercise the core's asynchronous inflate seam, not the codec.
            Opening::Inflated => Some(b"worker-owned compressed bytes".as_slice()),
        };
        let advertisement = advertise_response_segment_from(
            &mut sender,
            command,
            request,
            &body,
            candidate,
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: 132,
            },
            at - 100,
        );
        let pull = feed(receiver, &advertisement, at);
        assert_eq!(pull.frames.len(), 1);
        let hash = receiver
            .incoming_assemblies
            .original_hash(&link_id())
            .unwrap();
        let mut served = feed(&mut sender, &pull.frames[0].1, at + 10);
        assert_eq!(served.frames.len(), 1);
        Self {
            sender,
            command,
            request,
            hash,
            body,
            part: served.frames.remove(0).1,
        }
    }

    fn complete_current(mut self, receiver: &mut EngineState<TestStorageLayout>) {
        let mut first = feed(receiver, &self.part, 2_500);
        for (_, proof) in first.frames.drain(..) {
            feed(&mut self.sender, &proof, 2_600);
        }
        assert_eq!(
            first,
            InboundCapture {
                response_segments: std::vec![(self.command, self.request, 1, self.body.to_vec())],
                ..InboundCapture::default()
            }
        );
        let last = advertise_response_segment_from(
            &mut self.sender,
            CommandId(90),
            self.request,
            b"tail",
            None,
            ResourceSegment {
                index: 2,
                total_segments: 2,
                total_data_bytes: 132,
            },
            2_700,
        );
        let pull = feed(receiver, &last, 2_800);
        assert_eq!(pull.frames.len(), 1);
        let served = feed(&mut self.sender, &pull.frames[0].1, 2_900);
        assert_eq!(served.frames.len(), 1);
        let mut complete = feed(receiver, &served.frames[0].1, 3_000);
        for (_, proof) in complete.frames.drain(..) {
            feed(&mut self.sender, &proof, 3_100);
        }
        assert_eq!(
            complete,
            InboundCapture {
                response_segments: std::vec![(self.command, self.request, 2, b"tail".to_vec())],
                settlements: std::vec![(
                    self.command,
                    Settlement::SendRequest(Ok(PacketReceiptDelivered {
                        rtt: RttMillis::new(1_200),
                        evidence: DeliveryEvidence::Response,
                    }))
                )],
                ..InboundCapture::default()
            }
        );
        assert!(receiver.receipts.is_empty());
        assert!(receiver.incoming_resources.is_empty());
        assert_eq!(receiver.incoming_assemblies.original_hash(&link_id()), None);
    }
}

fn assert_old_failed_only(
    receiver: &EngineState<TestStorageLayout>,
    old: &FirstSegment,
    current: &FirstSegment,
    capture: InboundCapture,
    cause: ResourceFailureCause,
) {
    assert_eq!(
        capture,
        InboundCapture {
            failed: std::vec![(old.hash, cause)],
            settlements: std::vec![(
                old.command,
                Settlement::SendRequest(Err(SendRequestFailure::ResponseTransferFailed(cause)))
            )],
            ..InboundCapture::default()
        }
    );
    assert_eq!(receiver.incoming_resources.len(), 1);
    assert!(receiver
        .incoming_resources
        .lookup(&link_id(), &old.hash)
        .is_none());
    assert_eq!(
        receiver.incoming_assemblies.fit(
            &link_id(),
            &current.hash,
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: 132
            },
            AssemblyCorrelation::Response(current.request)
        ),
        SegmentFit::Expected
    );
    assert!(!receiver
        .receipts
        .has_pending_request(&link_id(), old.request));
    assert!(receiver
        .receipts
        .has_pending_request(&link_id(), current.request));
}

#[test]
fn a_replaced_split_transfer_cannot_publish_or_advance_the_current_chain() {
    for opening in [Opening::Inline, Opening::Inflated] {
        let mut receiver = engine_with_active_link();
        let old = FirstSegment::admit(&mut receiver, 42, opening, 2_000);
        // Park the old transfer at the inflate seam before replacing its chain.
        let inflated = receiver.incoming_resources.state(0).compression
            == crate::routing::links::resources::ResourceCompression::Bz2;
        if inflated {
            assert_eq!(
                feed(&mut receiver, &old.part, 2_200),
                InboundCapture::default()
            );
        }
        let current = FirstSegment::admit(&mut receiver, 43, Opening::Inline, 2_300);
        assert_ne!(old.hash, current.hash);
        let capture = if inflated {
            let mut capture = InboundCapture::default();
            let mut ready = std::collections::VecDeque::new();
            receiver.resume_resource_decompression(
                ResourceDecompressionCompleted {
                    link_id: link_id(),
                    hash: old.hash,
                    plaintext: &old.body,
                },
                InstantMillis(2_400),
                &mut |bytes| bytes.fill(0xC9),
                &mut |reaction| capture_inbound_reaction(reaction, &mut capture, &mut ready),
            );
            assert!(ready.is_empty());
            capture
        } else {
            feed(&mut receiver, &old.part, 2_400)
        };
        assert_old_failed_only(
            &receiver,
            &old,
            &current,
            capture,
            ResourceFailureCause::TransferCorrupt,
        );
        current.complete_current(&mut receiver);
    }
}

#[test]
fn cancelling_a_replaced_split_transfer_preserves_the_current_chain() {
    let mut receiver = engine_with_active_link();
    let old = FirstSegment::admit(&mut receiver, 42, Opening::Inline, 2_000);
    let current = FirstSegment::admit(&mut receiver, 43, Opening::Inline, 2_300);
    let mut cancel = [0; BROADCAST_MTU];
    let length = write_link_packet(
        &link_id(),
        &link_key(),
        BROADCAST_MTU,
        WireContext::ResourceInitiatorCancel,
        old.hash.as_bytes(),
        &[0xD5; 16],
        &mut cancel,
    )
    .unwrap();
    let capture = feed(&mut receiver, &cancel[..length], 2_400);
    assert_old_failed_only(
        &receiver,
        &old,
        &current,
        capture,
        ResourceFailureCause::CancelledBySender,
    );
    current.complete_current(&mut receiver);
}

#[test]
fn stale_transfers_cannot_fail_a_replacement_answering_the_same_request() {
    for cause in [
        ResourceFailureCause::TransferCorrupt,
        ResourceFailureCause::CancelledBySender,
    ] {
        let mut receiver = engine_with_active_link();
        let old = FirstSegment::admit(&mut receiver, 42, Opening::Inline, 2_000);
        let current = FirstSegment::admit_response(
            &mut receiver,
            old.command,
            old.request,
            43,
            Opening::Inline,
            2_300,
        );
        assert_ne!(old.hash, current.hash);
        let deadline = receiver
            .receipts
            .pending_request_deadline(&link_id(), old.request);
        let capture = if cause == ResourceFailureCause::TransferCorrupt {
            feed(&mut receiver, &old.part, 2_400)
        } else {
            let mut cancel = [0; BROADCAST_MTU];
            let length = write_link_packet(
                &link_id(),
                &link_key(),
                BROADCAST_MTU,
                WireContext::ResourceInitiatorCancel,
                old.hash.as_bytes(),
                &[0xD5; 16],
                &mut cancel,
            )
            .unwrap();
            feed(&mut receiver, &cancel[..length], 2_400)
        };
        assert_eq!(
            capture,
            InboundCapture {
                failed: std::vec![(old.hash, cause)],
                ..InboundCapture::default()
            }
        );
        assert_eq!(
            receiver
                .receipts
                .pending_request_deadline(&link_id(), old.request),
            deadline
        );
        assert_eq!(receiver.incoming_resources.len(), 1);
        assert!(receiver
            .incoming_resources
            .lookup(&link_id(), &old.hash)
            .is_none());
        assert_eq!(
            receiver.incoming_assemblies.fit(
                &link_id(),
                &current.hash,
                ResourceSegment {
                    index: 1,
                    total_segments: 2,
                    total_data_bytes: 132
                },
                AssemblyCorrelation::Response(current.request)
            ),
            SegmentFit::Expected
        );
        current.complete_current(&mut receiver);
    }
}
