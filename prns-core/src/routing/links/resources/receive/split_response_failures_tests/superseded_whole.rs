use super::*;
use crate::routing::links::resources::control::write_cancel_plaintext;

enum WholeFailure {
    MalformedMetadata,
    SenderCancellation,
    RetryExhaustion,
}

#[test]
fn failed_preadmitted_whole_leaves_the_real_split_response_able_to_finish() {
    for failure in [
        WholeFailure::MalformedMetadata,
        WholeFailure::SenderCancellation,
        WholeFailure::RetryExhaustion,
    ] {
        let mut receiver = engine_with_active_link();
        let request = track_pending_request(&mut receiver, CommandId(42), 1_800, 20_000);
        // Independent authenticated peer state bypasses sender-side LinkBusy.
        let mut whole_sender = engine_with_active_link();
        let whole = advertise_response_segment_from(
            &mut whole_sender,
            CommandId(7),
            request,
            &[0xFF; 128],
            None,
            ResourceSegment::whole(128),
            1_900,
        );
        let whole = rewrite_advertisement(&whole, |ad| ad.flags.has_metadata = true);
        let whole_pull = feed(&mut receiver, &whole, 2_000);
        assert_eq!(whole_pull.frames.len(), 1);
        let whole_hash = receiver
            .incoming_resources
            .first_hash_for_link(&link_id())
            .unwrap();

        let mut split_sender = engine_with_active_link();
        let first = [0xA7; 128];
        let last = [0xB8; 128];
        let advertisement = advertise_response_segment_from(
            &mut split_sender,
            CommandId(20),
            request,
            &first,
            None,
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: 256,
            },
            2_050,
        );
        let pull = feed(&mut receiver, &advertisement, 2_100);
        assert_eq!(pull.frames.len(), 1);
        let original = receiver
            .incoming_assemblies
            .original_hash(&link_id())
            .unwrap();
        let served = feed(&mut split_sender, &pull.frames[0].1, 2_150);
        assert_eq!(served.frames.len(), 1);
        let mut completed = feed(&mut receiver, &served.frames[0].1, 2_200);
        for (_, proof) in completed.frames.drain(..) {
            feed(&mut split_sender, &proof, 2_300);
        }
        assert_eq!(
            completed,
            InboundCapture {
                response_segments: std::vec![(CommandId(42), request, 1, first.to_vec())],
                ..InboundCapture::default()
            }
        );
        let deadline = receiver
            .receipts
            .pending_request_deadline(&link_id(), request);
        let mut delivery = Delivery::default();
        let cause = match failure {
            WholeFailure::MalformedMetadata => {
                let served = feed(&mut whole_sender, &whole_pull.frames[0].1, 2_400);
                for (_, frame) in &served.frames {
                    assert!(delivery
                        .absorb(feed(&mut receiver, frame, 2_500))
                        .is_empty());
                }
                ResourceFailureCause::MetadataOverrun
            }
            WholeFailure::SenderCancellation => {
                let mut plaintext = [0; 32];
                let len = write_cancel_plaintext(&whole_hash, &mut plaintext).unwrap();
                let mut frame = [0; BROADCAST_MTU];
                let len = write_link_packet(
                    &link_id(),
                    &link_key(),
                    BROADCAST_MTU,
                    WireContext::ResourceInitiatorCancel,
                    &plaintext[..len],
                    &[0xC3; 16],
                    &mut frame,
                )
                .unwrap();
                assert!(delivery
                    .absorb(feed(&mut receiver, &frame[..len], 2_500))
                    .is_empty());
                ResourceFailureCause::CancelledBySender
            }
            WholeFailure::RetryExhaustion => {
                let index = receiver
                    .incoming_resources
                    .lookup(&link_id(), &whole_hash)
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
        };
        assert_eq!(
            delivery,
            Delivery {
                failures: std::vec![(whole_hash, cause)],
                ..Delivery::default()
            }
        );
        assert_eq!(
            receiver
                .receipts
                .pending_request_deadline(&link_id(), request),
            deadline
        );
        assert_eq!(
            receiver.incoming_assemblies.original_hash(&link_id()),
            Some(original)
        );
        assert!(receiver.incoming_resources.is_empty());

        let advertisement = advertise_response_segment_from(
            &mut split_sender,
            CommandId(21),
            request,
            &last,
            None,
            ResourceSegment {
                index: 2,
                total_segments: 2,
                total_data_bytes: 256,
            },
            2_600,
        );
        let pull = feed(&mut receiver, &advertisement, 2_700);
        assert_eq!(pull.frames.len(), 1);
        let served = feed(&mut split_sender, &pull.frames[0].1, 2_750);
        assert_eq!(served.frames.len(), 1);
        let mut completed = feed(&mut receiver, &served.frames[0].1, 2_800);
        for (_, proof) in completed.frames.drain(..) {
            feed(&mut split_sender, &proof, 2_900);
        }
        assert_eq!(
            completed,
            InboundCapture {
                response_segments: std::vec![(CommandId(42), request, 2, last.to_vec())],
                settlements: std::vec![(
                    CommandId(42),
                    Settlement::SendRequest(Ok(PacketReceiptDelivered {
                        rtt: RttMillis::new(1_000),
                        evidence: DeliveryEvidence::Response,
                    }))
                )],
                ..InboundCapture::default()
            }
        );
        assert_retired(&receiver, request);
        assert!(split_sender.outgoing_resources.is_empty());
    }
}
