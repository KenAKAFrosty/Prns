use super::tests_support::*;
mod stream_sizes;
mod superseded_whole;
mod value_limits;
use crate::engine::test_support::{filled_frame, TestStorageLayout};
use crate::engine::{
    CommandId, DeliveryEvidence, Directive, EngineReaction, EngineState, InstantMillis, Journaled,
    NoOwedWork, PacketReceiptDelivered, ResourceDecompressionCompleted, SendRequestFailure,
    Settlement,
};
use crate::routing::links::data::write_link_packet;
use crate::routing::links::request::{write_response_plaintext, RequestId, RESPONSE_WIRE_OVERHEAD};
use crate::routing::links::resources::assembly::AssemblyCorrelation;
use crate::routing::links::resources::{ResourceFailureCause, ResourceHash, ResourceSegment};
use crate::units::RttMillis;
use crate::wire::{WireContext, BROADCAST_MTU};

enum Opening {
    Uncompressed,
    Inflated,
}

impl Opening {
    fn candidate(&self) -> Option<&'static [u8]> {
        match self {
            Self::Uncompressed => None,
            // This exercises the core's worker-completion seam, not a bzip2 codec.
            Self::Inflated => Some(b"worker-owned compressed bytes"),
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Delivery {
    chunks: std::vec::Vec<(CommandId, RequestId, u64, std::vec::Vec<u8>)>,
    settlements: std::vec::Vec<(CommandId, Settlement)>,
    failures: std::vec::Vec<(ResourceHash, ResourceFailureCause)>,
}

impl Delivery {
    fn record<Work>(&mut self, reaction: EngineReaction<'_, Work>) {
        match reaction {
            EngineReaction::Journaled(Journaled::ResponseSegmentReceived {
                command_id,
                request_id,
                segment_index,
                data,
                ..
            }) => self
                .chunks
                .push((command_id, request_id, segment_index, data.to_vec())),
            EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                assert!(self.settlements.is_empty(), "one settlement per request");
                self.settlements.push((id, settlement));
            }
            EngineReaction::Journaled(Journaled::ResourceFailed { hash, cause, .. }) => {
                assert!(self.failures.is_empty(), "one failure per transfer");
                self.failures.push((hash, cause));
            }
            EngineReaction::Journaled(
                Journaled::ResourceAssembled { .. }
                | Journaled::ResourceReceived { .. }
                | Journaled::ResourceSegmentReceived { .. }
                | Journaled::ResponseReceived { .. },
            ) => panic!("no alternate delivery lane"),
            _ => {}
        }
    }

    fn absorb(
        &mut self,
        capture: InboundCapture,
    ) -> std::vec::Vec<(crate::interfaces::InterfaceId, std::vec::Vec<u8>)> {
        assert!(
            capture.assembled.is_empty()
                && capture.received.is_empty()
                && capture.segments.is_empty()
                && capture.responses.is_empty()
        );
        self.chunks.extend(capture.response_segments);
        self.settlements.extend(capture.settlements);
        self.failures.extend(capture.failed);
        capture.frames
    }
}

fn expected_failure(hash: ResourceHash, cause: ResourceFailureCause) -> Delivery {
    Delivery {
        chunks: std::vec![],
        settlements: std::vec![(
            CommandId(42),
            Settlement::SendRequest(Err(SendRequestFailure::ResponseTransferFailed(cause),))
        )],
        failures: std::vec![(hash, cause)],
    }
}

fn assert_retired(receiver: &EngineState<TestStorageLayout>, request: RequestId) {
    assert!(!receiver.receipts.has_pending_request(&link_id(), request));
    assert!(receiver.incoming_resources.is_empty());
    assert_eq!(receiver.incoming_resources.active_buffer_bytes(), 0);
    assert!(receiver.pending_resource_offers.is_empty());
    assert_eq!(receiver.incoming_assemblies.original_hash(&link_id()), None);
}

fn finish_segment(
    receiver: &mut EngineState<TestStorageLayout>,
    parts: &[(crate::interfaces::InterfaceId, std::vec::Vec<u8>)],
    plaintext: &[u8],
    opening: &Opening,
    at: u64,
    delivery: &mut Delivery,
) -> std::vec::Vec<(crate::interfaces::InterfaceId, std::vec::Vec<u8>)> {
    let mut proofs = std::vec::Vec::new();
    for (_, frame) in parts {
        proofs.extend(delivery.absorb(feed(receiver, frame, at)));
    }
    if let Opening::Inflated = opening {
        let hash = receiver
            .incoming_resources
            .first_hash_for_link(&link_id())
            .unwrap();
        receiver.resume_resource_decompression(
            ResourceDecompressionCompleted {
                link_id: link_id(),
                hash,
                plaintext,
            },
            InstantMillis(at + 10),
            &mut |bytes| bytes.fill(0xC9),
            &mut |reaction: EngineReaction<'_, NoOwedWork>| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                    proofs.push((target, filled_frame(fill).unwrap()));
                }
                reaction => delivery.record(reaction),
            },
        );
    }
    proofs
}

enum Envelope {
    Matching,
    Mismatched,
}

fn check_split_envelope(envelope: Envelope) {
    for opening in [Opening::Uncompressed, Opening::Inflated] {
        let mut receiver = engine_with_active_link();
        let request = track_pending_request(&mut receiver, CommandId(42), 1_800, 20_000);
        let mut sender = engine_with_active_link();
        let mut first = std::vec![0; RESPONSE_WIRE_OVERHEAD + 128];
        let enclosed = match envelope {
            Envelope::Matching => request,
            Envelope::Mismatched => RequestId([0xF3; 16]),
        };
        write_response_plaintext(&enclosed, &[0xA7; 128], &mut first).unwrap();
        let tail = [0xB8; 128];
        let mut delivery = Delivery::default();
        let mut first_hash = None;
        for (index, data) in [(1, first.as_slice()), (2, tail.as_slice())] {
            let at = 2_000 + (index - 1) * 1_000;
            let advertisement = advertise_response_segment_from(
                &mut sender,
                CommandId(20 + index),
                request,
                data,
                opening.candidate(),
                ResourceSegment {
                    index,
                    total_segments: 2,
                    total_data_bytes: (first.len() + tail.len()) as u64,
                },
                at,
            );
            let pull = feed(&mut receiver, &advertisement, at + 100);
            if pull.frames.is_empty() {
                assert_eq!(index, 2, "only the late continuation may be refused");
                assert!(delivery.absorb(pull).is_empty());
                continue;
            }
            assert_eq!(pull.frames.len(), 1);
            if index == 1 {
                first_hash = receiver.incoming_resources.first_hash_for_link(&link_id());
            }
            let served = feed(&mut sender, &pull.frames[0].1, at + 200);
            assert!(!served.frames.is_empty());
            let proofs = finish_segment(
                &mut receiver,
                &served.frames,
                data,
                &opening,
                at + 300,
                &mut delivery,
            );
            for (_, proof) in proofs {
                feed(&mut sender, &proof, at + 400);
            }
            for (_, part) in &served.frames {
                let mut replay = Delivery::default();
                replay.absorb(feed(&mut receiver, part, at + 500));
                assert_eq!(replay, Delivery::default());
            }
        }
        let expected = match envelope {
            Envelope::Mismatched => {
                expected_failure(first_hash.unwrap(), ResourceFailureCause::TransferCorrupt)
            }
            Envelope::Matching => Delivery {
                chunks: std::vec![
                    (CommandId(42), request, 1, std::vec![0xA7; 128]),
                    (CommandId(42), request, 2, tail.to_vec())
                ],
                settlements: std::vec![(
                    CommandId(42),
                    Settlement::SendRequest(Ok(PacketReceiptDelivered {
                        rtt: RttMillis::new(match opening {
                            Opening::Uncompressed => 1_500,
                            Opening::Inflated => 1_510,
                        }),
                        evidence: DeliveryEvidence::Response,
                    }))
                )],
                failures: std::vec![],
            },
        };
        assert_eq!(delivery, expected);
        assert_retired(&receiver, request);
    }
}

#[test]
fn mismatched_first_envelope_cannot_complete_successfully_with_only_the_tail() {
    check_split_envelope(Envelope::Mismatched);
}

#[test]
fn matching_envelope_delivers_every_segment_and_settles_once() {
    check_split_envelope(Envelope::Matching);
}

#[test]
fn malformed_metadata_retires_the_failed_split_chain_on_both_open_paths() {
    for opening in [Opening::Uncompressed, Opening::Inflated] {
        let mut receiver = engine_with_active_link();
        let request = track_pending_request(&mut receiver, CommandId(42), 1_800, 20_000);
        let mut sender = engine_with_active_link();
        let body = [0xFF; 128];
        let advertisement = advertise_response_segment_from(
            &mut sender,
            CommandId(7),
            request,
            &body,
            opening.candidate(),
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: 256,
            },
            1_900,
        );
        let advertisement =
            rewrite_advertisement(&advertisement, |ad| ad.flags.has_metadata = true);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        let hash = receiver
            .incoming_resources
            .first_hash_for_link(&link_id())
            .unwrap();
        let served = feed(&mut sender, &pull.frames[0].1, 2_100);
        let mut delivery = Delivery::default();
        assert!(finish_segment(
            &mut receiver,
            &served.frames,
            &body,
            &opening,
            2_200,
            &mut delivery
        )
        .is_empty());
        assert_eq!(
            delivery,
            expected_failure(hash, ResourceFailureCause::MetadataOverrun)
        );
        assert_retired(&receiver, request);
    }
}

#[test]
fn a_cancelled_split_response_releases_its_assembly_and_settles_once() {
    let mut receiver = engine_with_active_link();
    let request = track_pending_request(&mut receiver, CommandId(42), 1_800, 20_000);
    let advertisement = advertise_response_segment_from(
        &mut engine_with_active_link(),
        CommandId(7),
        request,
        &[0xA7; 128],
        None,
        ResourceSegment {
            index: 1,
            total_segments: 2,
            total_data_bytes: 256,
        },
        1_900,
    );
    feed(&mut receiver, &advertisement, 2_000);
    let hash = receiver
        .incoming_resources
        .first_hash_for_link(&link_id())
        .unwrap();
    let mut frame = [0; BROADCAST_MTU];
    for (attempt, iv) in [0xE1, 0xE2].into_iter().enumerate() {
        let len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceInitiatorCancel,
            hash.as_bytes(),
            &[iv; 16],
            &mut frame,
        )
        .unwrap();
        let mut delivery = Delivery::default();
        delivery.absorb(feed(&mut receiver, &frame[..len], 2_100 + attempt as u64));
        assert_eq!(
            delivery,
            if attempt == 0 {
                expected_failure(hash, ResourceFailureCause::CancelledBySender)
            } else {
                Delivery::default()
            }
        );
    }
    assert_retired(&receiver, request);
}

#[test]
fn preadmitted_whole_failure_preserves_a_later_split_claim() {
    for opening in [Opening::Uncompressed, Opening::Inflated] {
        let mut receiver = engine_with_active_link();
        let request = track_pending_request(&mut receiver, CommandId(42), 1_800, 20_000);
        let previous = ResourceHash::new([0x31; 32]);
        let mut sender = engine_with_active_link();
        let body = [0xFF; 128];
        let advertisement = advertise_response_segment_from(
            &mut sender,
            CommandId(7),
            request,
            &body,
            opening.candidate(),
            ResourceSegment::whole(body.len() as u64),
            1_900,
        );
        let advertisement =
            rewrite_advertisement(&advertisement, |ad| ad.flags.has_metadata = true);
        let pull = feed(&mut receiver, &advertisement, 2_000);
        // The whole transfer was admitted before the split acquired ownership.
        receiver.incoming_assemblies.begin(
            link_id(),
            previous,
            2,
            256,
            AssemblyCorrelation::Response(request),
        );
        let hash = receiver
            .incoming_resources
            .first_hash_for_link(&link_id())
            .unwrap();
        let served = feed(&mut sender, &pull.frames[0].1, 2_100);
        let mut delivery = Delivery::default();
        finish_segment(
            &mut receiver,
            &served.frames,
            &body,
            &opening,
            2_200,
            &mut delivery,
        );
        assert_eq!(
            delivery,
            Delivery {
                failures: std::vec![(hash, ResourceFailureCause::MetadataOverrun)],
                ..Delivery::default()
            }
        );
        assert_eq!(
            receiver.incoming_assemblies.original_hash(&link_id()),
            Some(previous)
        );
        assert!(receiver.incoming_resources.is_empty());
        assert!(receiver.receipts.has_pending_request(&link_id(), request));
    }
}

enum Abandonment {
    RetryExhaustion,
    InvalidHashmap,
}

#[test]
fn deadline_and_hashmap_failures_also_release_the_split_assembly() {
    use crate::routing::links::resources::advertisement::write_hashmap_update_plaintext;
    use crate::routing::links::resources::table::ApplyHashmapUpdateError;
    for abandonment in [Abandonment::RetryExhaustion, Abandonment::InvalidHashmap] {
        let mut receiver = engine_with_active_link();
        let request = track_pending_request(&mut receiver, CommandId(42), 1_800, 20_000);
        let advertisement = advertise_response_segment_from(
            &mut engine_with_active_link(),
            CommandId(7),
            request,
            &[0xA7; 128],
            None,
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: 256,
            },
            1_900,
        );
        feed(&mut receiver, &advertisement, 2_000);
        let hash = receiver
            .incoming_resources
            .first_hash_for_link(&link_id())
            .unwrap();
        let mut delivery = Delivery::default();
        let cause = match abandonment {
            Abandonment::RetryExhaustion => {
                let index = receiver
                    .incoming_resources
                    .lookup(&link_id(), &hash)
                    .unwrap();
                receiver.incoming_resources.state_mut(index).retries_left = 0;
                receiver
                    .incoming_resources
                    .set_timeout_at(index, Some(InstantMillis(2_500)));
                receiver.fire_due_resource_deadlines(
                    InstantMillis(2_500),
                    &mut |bytes| bytes.fill(0xA9),
                    &mut |reaction| delivery.record(reaction),
                );
                ResourceFailureCause::RetriesExhausted
            }
            Abandonment::InvalidHashmap => {
                let mut plaintext = [0; BROADCAST_MTU];
                let len =
                    write_hashmap_update_plaintext(&hash, 5, &[0xAA; 4], &mut plaintext).unwrap();
                let mut frame = [0; BROADCAST_MTU];
                let len = write_link_packet(
                    &link_id(),
                    &link_key(),
                    BROADCAST_MTU,
                    WireContext::ResourceHashUpdate,
                    &plaintext[..len],
                    &[0xE1; 16],
                    &mut frame,
                )
                .unwrap();
                delivery.absorb(feed(&mut receiver, &frame[..len], 2_500));
                ResourceFailureCause::RefusedHashmapUpdate(ApplyHashmapUpdateError::BeyondPartCount)
            }
        };
        assert_eq!(delivery, expected_failure(hash, cause));
        assert_retired(&receiver, request);
        let mut later = Delivery::default();
        receiver.fire_due_resource_deadlines(
            InstantMillis(40_000),
            &mut |bytes| bytes.fill(0xA9),
            &mut |reaction| later.record(reaction),
        );
        assert_eq!(later, Delivery::default());
    }
}
