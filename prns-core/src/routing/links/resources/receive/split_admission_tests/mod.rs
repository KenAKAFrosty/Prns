use super::tests_support::*;
use crate::routing::links::resources::assembly::AssemblyBytes;
mod competing_whole;
mod refusal;
use crate::engine::test_support::{filled_frame, TestStorageLayout};
use crate::engine::{
    CommandId, DeliveryEvidence, Directive, EngineReaction, EngineState, InstantMillis, Journaled,
    PacketReceiptDelivered, Settlement,
};
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::assembly::{AssemblyCorrelation, AssemblyProgress};
use crate::routing::links::resources::{ResourceHash, ResourceSegment};
use crate::units::RttMillis;

const REQUEST: CommandId = CommandId(42);
const SEGMENT_BYTES: usize = 128;
const FIRST: [u8; SEGMENT_BYTES] = [0xA7; SEGMENT_BYTES];
const LAST: [u8; SEGMENT_BYTES] = [0xB8; SEGMENT_BYTES];

struct SplitResponse<S: crate::storage::StorageLayout = TestStorageLayout> {
    receiver: EngineState<S>,
    sender: EngineState<S>,
    request: RequestId,
    original: ResourceHash,
    continuation: std::vec::Vec<u8>,
}

impl SplitResponse {
    fn after_first_segment() -> Self {
        Self::with_limit(crate::units::ByteLimit::Unlimited)
    }

    fn with_limit(limit: crate::units::ByteLimit) -> Self {
        let mut receiver = engine_with_active_link();
        let request =
            track_pending_request_with_limit(&mut receiver, REQUEST, 1_800, 20_000, limit);
        Self::from_pending(receiver, request)
    }
}

impl<S: crate::storage::StorageLayout> SplitResponse<S> {
    fn from_pending(mut receiver: EngineState<S>, request: RequestId) -> Self {
        let retained_transfers = receiver.incoming_resources.len();
        let mut sender = active_engine::<S>();
        let first = advertise_response_segment_from(
            &mut sender,
            CommandId(20),
            request,
            &FIRST,
            None,
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: (2 * SEGMENT_BYTES) as u64,
            },
            1_900,
        );
        let pull = feed(&mut receiver, &first, 2_000);
        assert_eq!(pull.frames.len(), 1);
        let original = receiver
            .incoming_assemblies
            .original_hash(&link_id())
            .unwrap();
        let mut completed = serve_pull(&mut sender, &mut receiver, &pull.frames[0].1, 2_100);
        for (_, proof) in completed.frames.drain(..) {
            feed(&mut sender, &proof, 2_200);
        }
        assert_eq!(
            completed,
            InboundCapture {
                response_segments: std::vec![(REQUEST, request, 1, FIRST.to_vec())],
                ..InboundCapture::default()
            }
        );
        assert_eq!(receiver.incoming_resources.len(), retained_transfers);
        let continuation = advertise_response_segment_from(
            &mut sender,
            CommandId(21),
            request,
            &LAST,
            None,
            ResourceSegment {
                index: 2,
                total_segments: 2,
                total_data_bytes: (2 * SEGMENT_BYTES) as u64,
            },
            2_300,
        );
        Self {
            receiver,
            sender,
            request,
            original,
            continuation,
        }
    }

    fn complete(&mut self, pull: &[u8]) {
        self.complete_at(pull, 3_000);
    }

    fn complete_at(&mut self, pull: &[u8], at: u64) {
        let mut completed = serve_pull(&mut self.sender, &mut self.receiver, pull, at);
        for (_, proof) in completed.frames.drain(..) {
            feed(&mut self.sender, &proof, at + 100);
        }
        assert_eq!(
            completed,
            InboundCapture {
                response_segments: std::vec![(REQUEST, self.request, 2, LAST.to_vec())],
                settlements: std::vec![(
                    REQUEST,
                    Settlement::SendRequest(Ok(PacketReceiptDelivered {
                        rtt: RttMillis::new(at - 1_800),
                        evidence: DeliveryEvidence::Response,
                    }))
                )],
                ..InboundCapture::default()
            }
        );
        assert!(!self
            .receiver
            .receipts
            .has_pending_request(&link_id(), self.request));
        assert!(self.receiver.incoming_resources.is_empty());
        assert!(self.receiver.pending_resource_offers.is_empty());
        assert_eq!(
            self.receiver.incoming_assemblies.original_hash(&link_id()),
            None
        );
    }
}

fn serve_pull<S: crate::storage::StorageLayout>(
    sender: &mut EngineState<S>,
    receiver: &mut EngineState<S>,
    pull: &[u8],
    at: u64,
) -> InboundCapture {
    let served = feed(sender, pull, at);
    assert_eq!(served.frames.len(), 1, "each fixture segment fits one part");
    feed(receiver, &served.frames[0].1, at)
}

#[test]
fn a_continuation_cannot_rewrite_the_original_segment_count() {
    for changed_total in [3, u64::MAX] {
        let mut response = SplitResponse::after_first_segment();
        let deadline = response
            .receiver
            .receipts
            .pending_request_deadline(&link_id(), response.request);
        let changed = rewrite_advertisement(&response.continuation, |ad| {
            ad.total_segments = changed_total;
        });
        assert_eq!(
            feed(&mut response.receiver, &changed, 2_400),
            InboundCapture::default()
        );
        assert!(response.receiver.incoming_resources.is_empty());
        assert!(response.receiver.pending_resource_offers.is_empty());
        assert_eq!(
            response
                .receiver
                .receipts
                .pending_request_deadline(&link_id(), response.request),
            deadline,
            "a refused advertisement does not claim or extend the request"
        );
        let pull = feed(&mut response.receiver, &response.continuation, 2_500);
        assert_eq!(pull.frames.len(), 1);
        response.complete(&pull.frames[0].1);
    }
}

enum QueuedChain {
    Unchanged,
    Removed,
    Replaced,
    Advanced,
    ChangedCount,
    ChangedStreamSize,
    ChangedCorrelation,
}

#[test]
fn a_continuation_cannot_rewrite_the_original_stream_size() {
    for changed_size in [0, 255, 257, u64::MAX] {
        let mut response = SplitResponse::after_first_segment();
        let deadline = response
            .receiver
            .receipts
            .pending_request_deadline(&link_id(), response.request);
        let changed =
            rewrite_advertisement(&response.continuation, |ad| ad.data_bytes = changed_size);
        assert_eq!(
            feed(&mut response.receiver, &changed, 2_400),
            InboundCapture::default()
        );
        assert!(response.receiver.incoming_resources.is_empty());
        assert_eq!(
            response
                .receiver
                .receipts
                .pending_request_deadline(&link_id(), response.request),
            deadline
        );
        let pull = feed(&mut response.receiver, &response.continuation, 2_500);
        assert_eq!(pull.frames.len(), 1);
        response.complete(&pull.frames[0].1);
    }
}

#[test]
fn continuations_cannot_retarget_a_request_or_change_correlation_kind() {
    use crate::crypto::Ed25519PublicKey;
    use crate::identity::IdentitySigningPublicKey;
    use crate::routing::dedup::PacketHash;
    use crate::routing::delivery::receipts::{OutstandingReceipt, ReceiptKind};
    use crate::units::ByteLimit;

    enum Change {
        OtherResponse,
        OversizedOtherResponse,
        Request,
        Unsolicited,
    }
    for change in [
        Change::OtherResponse,
        Change::OversizedOtherResponse,
        Change::Request,
        Change::Unsolicited,
    ] {
        let mut response = SplitResponse::after_first_segment();
        accept_everything(&mut response.receiver);
        let other_hash = PacketHash::new([0xE2; 32]);
        let other_request = RequestId::of_packet(&other_hash);
        let limit = match change {
            Change::OversizedOtherResponse => ByteLimit::Maximum(0),
            _ => ByteLimit::Unlimited,
        };
        assert_eq!(
            response.receiver.receipts.track(OutstandingReceipt {
                packet_hash: other_hash,
                command_id: CommandId(43),
                kind: ReceiptKind::request(
                    link_id(),
                    limit,
                    crate::engine::SendRequestIntent::Application
                ),
                peer_signing_key: IdentitySigningPublicKey::new(Ed25519PublicKey([0x99; 32])),
                sent_at: InstantMillis(1_800),
                timeout_at: InstantMillis(20_000),
            }),
            None
        );
        let deadlines = (
            response
                .receiver
                .receipts
                .pending_request_deadline(&link_id(), response.request),
            response
                .receiver
                .receipts
                .pending_request_deadline(&link_id(), other_request),
        );
        let changed = rewrite_advertisement(&response.continuation, |ad| match change {
            Change::OtherResponse | Change::OversizedOtherResponse => {
                ad.request_id = Some(other_request)
            }
            Change::Request => {
                ad.flags.is_response = false;
                ad.flags.is_request = true;
            }
            Change::Unsolicited => {
                ad.flags.is_response = false;
                ad.request_id = None;
            }
        });
        assert_eq!(
            feed(&mut response.receiver, &changed, 2_400),
            InboundCapture::default()
        );
        assert!(response.receiver.incoming_resources.is_empty());
        assert!(response.receiver.pending_resource_offers.is_empty());
        assert_eq!(
            (
                response
                    .receiver
                    .receipts
                    .pending_request_deadline(&link_id(), response.request),
                response
                    .receiver
                    .receipts
                    .pending_request_deadline(&link_id(), other_request),
            ),
            deadlines
        );
        let pull = feed(&mut response.receiver, &response.continuation, 2_500);
        assert_eq!(pull.frames.len(), 1);
        response.complete(&pull.frames[0].1);
        assert_eq!(
            response
                .receiver
                .receipts
                .pending_request_deadline(&link_id(), other_request),
            deadlines.1
        );
    }
}

#[test]
fn queued_continuations_are_revalidated_before_allocating_or_claiming_the_request() {
    for change in [
        QueuedChain::Unchanged,
        QueuedChain::Removed,
        QueuedChain::Replaced,
        QueuedChain::Advanced,
        QueuedChain::ChangedCount,
        QueuedChain::ChangedStreamSize,
        QueuedChain::ChangedCorrelation,
    ] {
        let mut response = SplitResponse::after_first_segment();
        accept_everything(&mut response.receiver);
        let mut blockers = std::vec::Vec::new();
        for byte in [0xC1, 0xC2] {
            let advertisement = advertisement_frame(&[byte; SEGMENT_BYTES], None);
            assert_eq!(
                feed(&mut response.receiver, &advertisement, 2_350)
                    .frames
                    .len(),
                1
            );
            let index = response.receiver.incoming_resources.len() - 1;
            blockers.push(*response.receiver.incoming_resources.hash_at(index));
        }
        let deadline = response
            .receiver
            .receipts
            .pending_request_deadline(&link_id(), response.request);
        assert_eq!(
            feed(&mut response.receiver, &response.continuation, 2_400),
            InboundCapture::default()
        );
        assert_eq!(response.receiver.pending_resource_offers.len(), 1);
        // Inject stale assembly snapshots at the promotion boundary while the
        // bounded transfer store is occupied.
        let expected_original = match change {
            QueuedChain::Unchanged => Some(response.original),
            QueuedChain::Removed => {
                response.receiver.incoming_assemblies.clear(&link_id());
                None
            }
            QueuedChain::Replaced => {
                let replacement = ResourceHash::new([0xD1; 32]);
                response.receiver.incoming_assemblies.begin(
                    link_id(),
                    replacement,
                    2,
                    256,
                    AssemblyCorrelation::Response(response.request),
                );
                Some(replacement)
            }
            QueuedChain::Advanced => {
                assert_eq!(
                    response.receiver.incoming_assemblies.advance(
                        &link_id(),
                        &response.original,
                        ResourceSegment {
                            index: 2,
                            total_segments: 2,
                            total_data_bytes: 256
                        },
                        AssemblyBytes {
                            stream: SEGMENT_BYTES as u64,
                            value: SEGMENT_BYTES as u64
                        },
                        AssemblyCorrelation::Response(response.request)
                    ),
                    Some(AssemblyProgress::Complete {
                        total_size_bytes: (2 * SEGMENT_BYTES) as u64
                    })
                );
                Some(response.original)
            }
            QueuedChain::ChangedCount | QueuedChain::ChangedStreamSize => {
                let count = if matches!(change, QueuedChain::ChangedCount) {
                    3
                } else {
                    2
                };
                let size = if matches!(change, QueuedChain::ChangedStreamSize) {
                    512
                } else {
                    256
                };
                response.receiver.incoming_assemblies.begin(
                    link_id(),
                    response.original,
                    count,
                    size,
                    AssemblyCorrelation::Response(response.request),
                );
                response.receiver.incoming_assemblies.advance(
                    &link_id(),
                    &response.original,
                    ResourceSegment {
                        index: 1,
                        total_segments: count,
                        total_data_bytes: size,
                    },
                    AssemblyBytes {
                        stream: SEGMENT_BYTES as u64,
                        value: SEGMENT_BYTES as u64,
                    },
                    AssemblyCorrelation::Response(response.request),
                );
                Some(response.original)
            }
            QueuedChain::ChangedCorrelation => {
                let changed = AssemblyCorrelation::Request(response.request);
                response.receiver.incoming_assemblies.begin(
                    link_id(),
                    response.original,
                    2,
                    256,
                    changed,
                );
                assert_eq!(
                    response.receiver.incoming_assemblies.advance(
                        &link_id(),
                        &response.original,
                        ResourceSegment {
                            index: 1,
                            total_segments: 2,
                            total_data_bytes: 256
                        },
                        AssemblyBytes {
                            stream: SEGMENT_BYTES as u64,
                            value: SEGMENT_BYTES as u64
                        },
                        changed
                    ),
                    Some(AssemblyProgress::Assembling)
                );
                Some(response.original)
            }
        };
        for hash in blockers {
            response
                .receiver
                .retire_incoming_resource(&link_id(), &hash);
        }
        let mut pulls = std::vec::Vec::new();
        response.receiver.fire_due_resource_deadlines(
            InstantMillis(2_500),
            &mut |bytes| bytes.fill(0xC5),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    pulls.push(filled_frame(fill).unwrap());
                }
                EngineReaction::Journaled(Journaled::CommandSettled { .. }) => {
                    panic!("revalidation must not settle another live request")
                }
                _ => {}
            },
        );
        assert!(response.receiver.pending_resource_offers.is_empty());
        match change {
            QueuedChain::Unchanged => {
                assert_eq!(pulls.len(), 1);
                response.complete(&pulls[0]);
            }
            QueuedChain::Removed
            | QueuedChain::Replaced
            | QueuedChain::Advanced
            | QueuedChain::ChangedCount
            | QueuedChain::ChangedStreamSize
            | QueuedChain::ChangedCorrelation => {
                assert!(
                    pulls.is_empty(),
                    "stale continuation must not start receiving"
                );
                assert!(response.receiver.incoming_resources.is_empty());
                assert_eq!(
                    response.receiver.incoming_resources.active_buffer_bytes(),
                    0
                );
                assert_eq!(
                    response
                        .receiver
                        .incoming_assemblies
                        .original_hash(&link_id()),
                    expected_original
                );
                assert_eq!(
                    response
                        .receiver
                        .receipts
                        .pending_request_deadline(&link_id(), response.request),
                    deadline
                );
            }
        }
    }
}
