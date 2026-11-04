use super::*;
use crate::engine::test_support::{filled_frame, TestStorageLayout};
use crate::engine::{
    DeliveryEvidence, Directive, InstantMillis, Journaled, LinkClosedReason,
    PacketReceiptDelivered, RequestResponseTimeout, SendRequestIntent, SendResourceRejection,
};
use crate::identity::IdentitySigningPublicKey;
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::receipts::{OutstandingReceipt, ReceiptKind, ReceiptTable};
use crate::routing::links::data::{write_link_packet, write_link_raw_packet};
use crate::routing::links::request::{write_response_plaintext, RequestId};
use crate::routing::links::resources::control::{write_proof_plaintext, PROOF_PLAINTEXT_LEN};
use crate::routing::links::resources::receive::tests_support::{
    active_engine, advertise_response_segment_from, feed, link_id, link_key,
};
use crate::routing::links::resources::{
    ResourceBody, ResourceHash, ResourceMetadata, ResourceSegment, ResourceSend,
};
use crate::units::{ByteLimit, DurationMillis, RttMillis};
use crate::wire::{PacketType, WireContext, BROADCAST_MTU};

const COMMAND: CommandId = CommandId(91);
const REQUEST: &[u8] = b"test-only packed request";
type Engine = EngineState<TestStorageLayout>;
type Settlements = Vec<(CommandId, Settlement)>;

fn send(command: CommandId) -> ResourceSend<'static> {
    ResourceSend {
        id: command,
        link_id: link_id(),
        body: ResourceBody {
            data: REQUEST,
            compressed_candidate: None,
            metadata: ResourceMetadata::None,
        },
        correlation: ResourceCorrelation::Request {
            id: RequestId::of_request_data(REQUEST),
            response_timeout: RequestResponseTimeout::Exact(DurationMillis(100)),
            maximum_response_bytes: ByteLimit::Maximum(256),
        },
    }
}

fn record<Work>(reaction: EngineReaction<'_, Work>, settlements: &mut Settlements) {
    match reaction {
        EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
            assert!(filled_frame(fill).is_some());
        }
        EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
            settlements.push((id, settlement));
        }
        _ => {}
    }
}

fn start(engine: &mut Engine) -> ResourceHash {
    let mut settled = Vec::new();
    let wake = engine.ingest_send_resource_into(
        &send(COMMAND),
        InstantMillis(10),
        &mut |bytes| bytes.fill(0x71),
        &mut |reaction| record(reaction, &mut settled),
    );
    assert_eq!(settled, []);
    assert_eq!(
        (wake.receipt_timeouts, wake.remote_control_pairing),
        (
            crate::engine::WakeSchedule::At(InstantMillis(110)),
            crate::engine::WakeSchedule::Idle
        )
    );
    assert_eq!(engine.receipts.len(), 1);
    *engine.outgoing_resources.hash_at(0).unwrap()
}

fn failed(failure: SendResourceFailure) -> (CommandId, Settlement) {
    (
        COMMAND,
        Settlement::SendRequest(Err(SendRequestFailure::RequestTransferFailed(failure))),
    )
}

fn encrypted(context: WireContext, data: &[u8]) -> Vec<u8> {
    let mut frame = vec![0; BROADCAST_MTU];
    let len = write_link_packet(
        &link_id(),
        &link_key(),
        BROADCAST_MTU,
        context,
        data,
        &[0x81; 16],
        &mut frame,
    )
    .unwrap();
    frame.truncate(len);
    frame
}

fn start_with_timeout(
    engine: &mut Engine,
    data: &[u8],
    response_timeout: RequestResponseTimeout,
) -> (ResourceHash, Vec<u8>) {
    use crate::routing::links::resources::advertisement::ResourceAdvertisement;
    use crate::wire::WirePacketHeader;

    let mut advertisement = None;
    engine.ingest_send_resource_into(
        &ResourceSend {
            body: ResourceBody {
                data,
                compressed_candidate: None,
                metadata: ResourceMetadata::None,
            },
            correlation: ResourceCorrelation::Request {
                id: RequestId::of_request_data(data),
                response_timeout,
                maximum_response_bytes: ByteLimit::Maximum(256),
            },
            ..send(COMMAND)
        },
        InstantMillis(10),
        &mut |bytes| bytes.fill(0x71),
        &mut |reaction| match reaction {
            EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                advertisement = filled_frame(fill);
            }
            EngineReaction::Journaled(Journaled::CommandSettled { .. }) => {
                panic!("the request should remain pending");
            }
            _ => {}
        },
    );
    let frame = advertisement.unwrap();
    let (_, sealed) = WirePacketHeader::parse(&frame).unwrap();
    let mut sealed = sealed.to_vec();
    let key = link_key();
    let plaintext = key.open_in_place(&mut sealed).unwrap();
    let advertisement = ResourceAdvertisement::parse(plaintext).unwrap();
    (advertisement.hash, advertisement.hashmap.to_vec())
}

fn upload_proof(engine: &Engine, hash: &ResourceHash) -> Vec<u8> {
    let index = engine.outgoing_resources.lookup(&link_id(), hash).unwrap();
    let mut proof = [0; PROOF_PLAINTEXT_LEN];
    write_proof_plaintext(
        hash,
        &engine.outgoing_resources.state(index).expected_proof,
        &mut proof,
    )
    .unwrap();
    let mut frame = vec![0; BROADCAST_MTU];
    let len = write_link_raw_packet(
        &link_id(),
        PacketType::Proof,
        WireContext::ResourceProof,
        BROADCAST_MTU,
        &proof,
        &mut frame,
    )
    .unwrap();
    frame.truncate(len);
    frame
}

#[test]
fn link_default_allows_healthy_upload_longer_than_the_response_wait() {
    use crate::routing::links::resources::control::{
        write_part_request_plaintext, PART_REQUEST_PLAINTEXT_CAP,
    };

    let mut engine = active_engine::<TestStorageLayout>();
    let data = [0x42; 2_200];
    let request = RequestId::of_request_data(&data);
    let (hash, names) = start_with_timeout(&mut engine, &data, RequestResponseTimeout::LinkDefault);
    assert_eq!(names.len(), 5 * 4);
    let mut settled = Vec::new();
    // A low-throughput upload remains healthy: every part request makes progress,
    // but the complete upload exceeds the Link's 13.75-second response allowance.
    for (index, name) in names.as_chunks::<4>().0.iter().enumerate() {
        let at = 1_000 + index as u64 * 4_000;
        engine.settle_timed_out_receipts(InstantMillis(at), &mut |r| record(r, &mut settled));
        engine.fire_due_resource_deadlines(InstantMillis(at), &mut |b| b.fill(0x72), &mut |r| {
            record(r, &mut settled)
        });
        assert_eq!(settled, []);
        let mut plaintext = [0; PART_REQUEST_PLAINTEXT_CAP];
        let len = write_part_request_plaintext(&hash, None, name, &mut plaintext).unwrap();
        let result = feed(
            &mut engine,
            &encrypted(WireContext::ResourceRequest, &plaintext[..len]),
            at,
        );
        assert_eq!(result.settlements, []);
        assert_eq!(result.frames.len(), 1);
    }
    let proof = upload_proof(&engine, &hash);
    assert_eq!(feed(&mut engine, &proof, 18_000).settlements, []);
    assert_eq!(
        engine.receipts.earliest_timeout_at(),
        Some(InstantMillis(31_750))
    );
    let mut response = [0; 64];
    let len = write_response_plaintext(&request, &[0xC4, 1, 0xAB], &mut response).unwrap();
    assert_eq!(
        feed(
            &mut engine,
            &encrypted(WireContext::Response, &response[..len]),
            19_000,
        )
        .settlements,
        [(
            COMMAND,
            Settlement::SendRequest(Ok(PacketReceiptDelivered {
                rtt: RttMillis::new(18_990),
                evidence: DeliveryEvidence::Response,
            }))
        )]
    );
    assert_eq!(
        (engine.receipts.len(), engine.outgoing_resources.len()),
        (0, 0)
    );
}

#[test]
fn link_default_upload_proof_arms_a_finite_response_deadline() {
    use crate::engine::test_support::routable_descriptor;
    use crate::engine::IngestIo;
    use crate::interfaces::{AttachedInterfaces, InboundPacket};
    use crate::routing::links::resources::receive::tests_support::lane;

    let mut engine = active_engine::<TestStorageLayout>();
    let (hash, _) = start_with_timeout(&mut engine, REQUEST, RequestResponseTimeout::LinkDefault);
    assert_eq!(engine.receipts.earliest_timeout_at(), None);
    let mut proof = upload_proof(&engine, &hash);
    let mut settled = Vec::new();
    let wake = engine.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(20),
            source_interface: lane(),
            bytes: &mut proof,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&[routable_descriptor(lane())]),
            now: InstantMillis(20),
            fill_random: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
            sink: &mut |r| record(r, &mut settled),
        },
    );
    assert_eq!(settled, []);
    assert_eq!(
        wake.receipt_timeouts,
        crate::engine::WakeSchedule::At(InstantMillis(13_770))
    );
    assert_eq!(
        engine.receipts.earliest_timeout_at(),
        Some(InstantMillis(13_770))
    );
    engine.settle_timed_out_receipts(InstantMillis(13_769), &mut |r| record(r, &mut settled));
    assert_eq!(settled, []);
    engine.settle_timed_out_receipts(InstantMillis(13_770), &mut |r| record(r, &mut settled));
    assert_eq!(
        settled,
        [(
            COMMAND,
            Settlement::SendRequest(Err(SendRequestFailure::Timeout))
        )]
    );
}

#[test]
fn explicit_request_response_deadlines_still_include_upload_time() {
    for (timeout, deadline) in [
        (RequestResponseTimeout::Exact(DurationMillis(100)), 110),
        (
            RequestResponseTimeout::LinkDefaultAtMost {
                maximum: DurationMillis(100),
            },
            110,
        ),
        (
            RequestResponseTimeout::LinkDefaultAtMost {
                maximum: DurationMillis(50_000),
            },
            13_760,
        ),
    ] {
        let mut engine = active_engine::<TestStorageLayout>();
        let (hash, _) = start_with_timeout(&mut engine, REQUEST, timeout);
        assert_eq!(
            engine.receipts.earliest_timeout_at(),
            Some(InstantMillis(deadline))
        );
        let proof = upload_proof(&engine, &hash);
        assert_eq!(feed(&mut engine, &proof, 100).settlements, []);
        assert_eq!(
            engine.receipts.earliest_timeout_at(),
            Some(InstantMillis(deadline))
        );
        let mut settled = Vec::new();
        engine.settle_timed_out_receipts(InstantMillis(deadline), &mut |r| record(r, &mut settled));
        assert_eq!(
            settled,
            [(
                COMMAND,
                Settlement::SendRequest(Err(SendRequestFailure::Timeout))
            )]
        );
    }
}

#[test]
fn link_default_upload_watchdog_still_settles_once() {
    let mut engine = active_engine::<TestStorageLayout>();
    let _ = start_with_timeout(&mut engine, REQUEST, RequestResponseTimeout::LinkDefault);
    engine.outgoing_resources.state_mut(0).retries_left = 0;
    let mut settled = Vec::new();
    engine.fire_due_resource_deadlines(InstantMillis(2_510), &mut |b| b.fill(0x72), &mut |r| {
        record(r, &mut settled)
    });
    assert_eq!(settled, [failed(SendResourceFailure::Timeout)]);
    assert_eq!(
        (engine.receipts.len(), engine.outgoing_resources.len()),
        (0, 0)
    );
    engine.settle_timed_out_receipts(InstantMillis(100_000), &mut |r| record(r, &mut settled));
    assert_eq!(settled.len(), 1);
}

#[test]
fn a_packet_response_before_upload_proof_cannot_rearm_a_completed_request() {
    let mut engine = active_engine::<TestStorageLayout>();
    let (hash, _) = start_with_timeout(&mut engine, REQUEST, RequestResponseTimeout::LinkDefault);
    let proof = upload_proof(&engine, &hash);
    let mut response = [0; 64];
    let len = write_response_plaintext(
        &RequestId::of_request_data(REQUEST),
        &[0xC4, 1, 0xAB],
        &mut response,
    )
    .unwrap();
    let received = feed(
        &mut engine,
        &encrypted(WireContext::Response, &response[..len]),
        100,
    );
    assert!(matches!(
        received.settlements.as_slice(),
        [(COMMAND, Settlement::SendRequest(Ok(_)))]
    ));
    assert_eq!(feed(&mut engine, &proof, 200).settlements, []);
    assert_eq!(
        (engine.receipts.len(), engine.outgoing_resources.len()),
        (0, 0)
    );
    assert_eq!(engine.receipts.earliest_timeout_at(), None);
}

#[test]
fn upload_completion_only_arms_its_own_still_uploading_receipt() {
    let mut engine = active_engine::<TestStorageLayout>();
    let _ = start_with_timeout(&mut engine, REQUEST, RequestResponseTimeout::LinkDefault);
    let correlation = engine.outgoing_resources.state(0).correlation;
    engine.complete_request_resource_upload(
        CommandId(92),
        &link_id(),
        correlation,
        InstantMillis(20),
    );
    assert_eq!(engine.receipts.earliest_timeout_at(), None);
    engine
        .receipts
        .claim_request_for_transfer(&link_id(), RequestId::of_request_data(REQUEST));
    engine.complete_request_resource_upload(COMMAND, &link_id(), correlation, InstantMillis(30));
    assert_eq!(engine.receipts.earliest_timeout_at(), None);
    assert_eq!(engine.receipts.len(), 1);
}

#[test]
fn mismatched_request_correlation_retains_a_finite_default_timeout() {
    let mut engine = active_engine::<TestStorageLayout>();
    let mut malformed = send(COMMAND);
    malformed.correlation = ResourceCorrelation::Request {
        id: RequestId([0xFF; 16]),
        response_timeout: RequestResponseTimeout::LinkDefault,
        maximum_response_bytes: ByteLimit::Maximum(256),
    };
    let mut settled = Vec::new();
    engine.ingest_send_resource_into(
        &malformed,
        InstantMillis(10),
        &mut |bytes| bytes.fill(0x71),
        &mut |r| record(r, &mut settled),
    );
    assert_eq!(settled, []);
    let hash = *engine.outgoing_resources.hash_at(0).unwrap();
    let proof = upload_proof(&engine, &hash);
    assert_eq!(feed(&mut engine, &proof, 20).settlements, []);
    assert_eq!(engine.outgoing_resources.len(), 0);
    assert_eq!(
        engine.receipts.earliest_timeout_at(),
        Some(InstantMillis(13_760))
    );
    engine.settle_timed_out_receipts(InstantMillis(13_760), &mut |r| record(r, &mut settled));
    assert_eq!(
        settled,
        [(
            COMMAND,
            Settlement::SendRequest(Err(SendRequestFailure::Timeout))
        )]
    );
    assert_eq!(engine.receipts.len(), 0);
}

#[test]
fn an_early_response_can_wait_for_capacity_without_an_upload_proof() {
    use crate::routing::links::resources::receive::tests_support::{
        accept_everything, advertisement_frame,
    };

    for release_capacity in [true, false] {
        let mut requester = active_engine::<TestStorageLayout>();
        let (hash, _) =
            start_with_timeout(&mut requester, REQUEST, RequestResponseTimeout::LinkDefault);
        let proof = upload_proof(&requester, &hash);
        accept_everything(&mut requester);
        for tag in [0x31, 0x32] {
            assert_eq!(
                feed(&mut requester, &advertisement_frame(&[tag; 40], None), 30)
                    .frames
                    .len(),
                1
            );
        }
        assert_eq!(requester.incoming_resources.len(), 2);
        let mut responder = active_engine::<TestStorageLayout>();
        let response = [0x42; 40];
        let advertisement = advertise_response_segment_from(
            &mut responder,
            CommandId(77),
            RequestId::of_request_data(REQUEST),
            &response,
            None,
            ResourceSegment::whole(response.len() as u64),
            40,
        );
        let queued = feed(&mut requester, &advertisement, 50);
        assert_eq!(queued.frames, []);
        assert_eq!(requester.pending_resource_offers.len(), 1);
        assert_eq!(
            requester.pending_resource_deadline(),
            Some(InstantMillis(5_050))
        );
        let mut settled = Vec::new();
        requester.fire_due_resource_deadlines(InstantMillis(60), &mut |b| b.fill(0x72), &mut |r| {
            record(r, &mut settled)
        });
        assert_eq!(settled, []);
        assert_eq!(requester.pending_resource_offers.len(), 1);
        if release_capacity {
            let filler = *requester.incoming_resources.hash_at(0);
            requester.incoming_resources.remove(&link_id(), &filler);
            let mut frames = Vec::new();
            requester.fire_due_resource_deadlines(
                InstantMillis(70),
                &mut |b| b.fill(0x72),
                &mut |r| match r {
                    EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                        frames.push(filled_frame(fill).unwrap());
                    }
                    other => record(other, &mut settled),
                },
            );
            assert_eq!(frames.len(), 1);
            assert_eq!(requester.outgoing_resources.len(), 0);
            assert_eq!(feed(&mut requester, &proof, 80).settlements, []);
            assert_eq!(requester.receipts.earliest_timeout_at(), None);
            let parts = feed(&mut responder, &frames[0], 85);
            assert_eq!(parts.frames.len(), 1);
            let received = feed(&mut requester, &parts.frames[0].1, 90);
            assert_eq!(
                received.responses,
                [(
                    COMMAND,
                    RequestId::of_request_data(REQUEST),
                    response.to_vec()
                )]
            );
            assert!(matches!(
                received.settlements.as_slice(),
                [(COMMAND, Settlement::SendRequest(Ok(_)))]
            ));
        } else {
            requester.fire_due_resource_deadlines(
                InstantMillis(5_050),
                &mut |b| b.fill(0x72),
                &mut |r| record(r, &mut settled),
            );
            assert_eq!(
                settled,
                [(
                    COMMAND,
                    Settlement::SendRequest(Err(SendRequestFailure::ResourceCapacity))
                )]
            );
        }
        assert_eq!(requester.pending_resource_offers.len(), 0);
        assert_eq!(requester.receipts.len(), 0);
    }
}

#[test]
fn admission_failure_settles_the_request_without_a_receipt() {
    let mut engine = active_engine::<TestStorageLayout>();
    let _ = start(&mut engine);
    let mut settled = Vec::new();
    engine.ingest_send_resource_into(
        &send(CommandId(92)),
        InstantMillis(11),
        &mut |bytes| bytes.fill(0x72),
        &mut |r| record(r, &mut settled),
    );
    assert_eq!(
        settled,
        [(
            CommandId(92),
            Settlement::SendRequest(Err(SendRequestFailure::RequestTransferFailed(
                SendResourceFailure::Rejected(SendResourceRejection::LinkBusy)
            )))
        )]
    );
    assert_eq!(
        engine
            .receipts
            .pending_request_command(&link_id(), RequestId::of_request_data(REQUEST)),
        Some(COMMAND)
    );
    assert_eq!(
        (engine.receipts.len(), engine.outgoing_resources.len()),
        (1, 1)
    );
}

#[test]
fn peer_rejection_removes_the_receipt_and_never_times_out_again() {
    let mut engine = active_engine::<TestStorageLayout>();
    let hash = start(&mut engine);
    let cancel = encrypted(WireContext::ResourceReceiverCancel, hash.as_bytes());
    assert_eq!(
        feed(&mut engine, &cancel, 20).settlements,
        [failed(SendResourceFailure::RejectedByPeer)]
    );
    assert_eq!(
        (
            engine.receipts.len(),
            engine.receipts.earliest_timeout_at(),
            engine.outgoing_resources.len()
        ),
        (0, None, 0)
    );
    assert_eq!(feed(&mut engine, &cancel, 21).settlements, []);
    let mut settled = Vec::new();
    engine.settle_timed_out_receipts(InstantMillis(100_000), &mut |r| record(r, &mut settled));
    assert_eq!(settled, []);
}

#[test]
fn receipt_timeout_wins_a_later_rejection_or_upload_watchdog() {
    for reject in [true, false] {
        let mut engine = active_engine::<TestStorageLayout>();
        let hash = start(&mut engine);
        let mut settled = Vec::new();
        engine.settle_timed_out_receipts(InstantMillis(110), &mut |r| record(r, &mut settled));
        assert_eq!(
            settled,
            [(
                COMMAND,
                Settlement::SendRequest(Err(SendRequestFailure::Timeout))
            )]
        );
        settled.clear();
        if reject {
            settled.extend(
                feed(
                    &mut engine,
                    &encrypted(WireContext::ResourceReceiverCancel, hash.as_bytes()),
                    111,
                )
                .settlements,
            );
        } else {
            let state = engine.outgoing_resources.state_mut(0);
            state.retries_left = 0;
            engine.fire_due_resource_deadlines(
                InstantMillis(100_000),
                &mut |bytes| bytes.fill(0x72),
                &mut |r| record(r, &mut settled),
            );
        }
        assert_eq!(settled, []);
        assert_eq!(
            (engine.receipts.len(), engine.outgoing_resources.len()),
            (0, 0)
        );
    }
}

#[test]
fn upload_watchdog_failure_retires_its_still_pending_response() {
    let mut engine = active_engine::<TestStorageLayout>();
    let _ = start(&mut engine);
    engine.receipts.arm_request_timeout(
        &link_id(),
        RequestId::of_request_data(REQUEST),
        InstantMillis(200_000),
    );
    engine.outgoing_resources.state_mut(0).retries_left = 0;
    let mut settled = Vec::new();
    engine.fire_due_resource_deadlines(
        InstantMillis(100_000),
        &mut |bytes| bytes.fill(0x72),
        &mut |r| record(r, &mut settled),
    );
    assert_eq!(settled, [failed(SendResourceFailure::Timeout)]);
    settled.clear();
    engine.settle_timed_out_receipts(InstantMillis(200_000), &mut |r| record(r, &mut settled));
    assert_eq!(
        (settled, engine.receipts.earliest_timeout_at()),
        (vec![], None)
    );
}

#[test]
fn upload_proof_is_not_response_success() {
    let mut engine = active_engine::<TestStorageLayout>();
    let hash = start(&mut engine);
    let mut proof = [0; PROOF_PLAINTEXT_LEN];
    write_proof_plaintext(
        &hash,
        &engine.outgoing_resources.state(0).expected_proof,
        &mut proof,
    )
    .unwrap();
    let mut frame = [0; BROADCAST_MTU];
    let len = write_link_raw_packet(
        &link_id(),
        PacketType::Proof,
        WireContext::ResourceProof,
        BROADCAST_MTU,
        &proof,
        &mut frame,
    )
    .unwrap();
    assert_eq!(feed(&mut engine, &frame[..len], 15).settlements, []);
    assert_eq!(
        (engine.receipts.len(), engine.outgoing_resources.len()),
        (1, 0)
    );
    let mut response = [0; 64];
    let len = write_response_plaintext(
        &RequestId::of_request_data(REQUEST),
        &[0xC4, 1, 0xAB],
        &mut response,
    )
    .unwrap();
    let frame = encrypted(WireContext::Response, &response[..len]);
    assert_eq!(
        feed(&mut engine, &frame, 20).settlements,
        [(
            COMMAND,
            Settlement::SendRequest(Ok(PacketReceiptDelivered {
                rtt: RttMillis::new(10),
                evidence: DeliveryEvidence::Response,
            }))
        )]
    );
    assert_eq!(engine.receipts.len(), 0);
}

#[test]
fn link_close_settles_advertised_and_still_building_requests_once() {
    for building in [false, true] {
        let mut engine = active_engine::<TestStorageLayout>();
        let mut reservation = None;
        if building {
            engine.request_resource_build(
                &send(COMMAND),
                ResourceSegment::whole(REQUEST.len() as u64),
                &mut |r| match r {
                    EngineReaction::Directive(Directive::Fulfill(
                        crate::engine::OwedWork::ResourceBuild(build),
                    )) => reservation = Some(build.reservation()),
                    _ => unreachable!("build must be reserved"),
                },
            );
        } else {
            let _ = start(&mut engine);
        }
        let mut settled = Vec::new();
        engine.retire_link(
            &link_id(),
            LinkClosedReason::Timeout,
            &mut |r: EngineReaction<'_>| record(r, &mut settled),
        );
        assert_eq!(settled, [failed(SendResourceFailure::LinkClosed)]);
        assert_eq!(
            (engine.receipts.len(), engine.outgoing_resources.len()),
            (0, 0)
        );
        settled.clear();
        if let Some(reservation) = reservation {
            engine.resume_resource_build_unavailable(reservation, &mut |r| record(r, &mut settled));
        }
        engine.settle_timed_out_receipts(InstantMillis(100_000), &mut |r| record(r, &mut settled));
        assert_eq!(settled, []);
    }
}

#[test]
fn resource_receipt_eviction_settles_the_displaced_request() {
    let mut engine = active_engine::<TestStorageLayout>();
    let capacity = <TestStorageLayout as StorageLayout>::Receipts::default().capacity();
    for index in 0..capacity {
        assert_eq!(
            engine.receipts.track(OutstandingReceipt {
                packet_hash: PacketHash::new([index as u8; 32]),
                command_id: CommandId(index as u64),
                kind: ReceiptKind::request(
                    link_id(),
                    ByteLimit::Maximum(256),
                    SendRequestIntent::Application
                ),
                peer_signing_key: IdentitySigningPublicKey::new(crate::crypto::Ed25519PublicKey(
                    [0x99; 32]
                )),
                sent_at: InstantMillis(index as u64),
                timeout_at: InstantMillis(200_000),
            }),
            None
        );
    }
    let mut settled = Vec::new();
    engine.ingest_send_resource_into(
        &send(COMMAND),
        InstantMillis(10),
        &mut |b| b.fill(0x71),
        &mut |r| record(r, &mut settled),
    );
    assert_eq!(
        settled,
        [(
            CommandId(0),
            Settlement::SendRequest(Err(SendRequestFailure::Culled))
        )]
    );
    assert_eq!(engine.receipts.len(), capacity);
    assert_eq!(
        engine
            .receipts
            .pending_request_command(&link_id(), RequestId::of_request_data(REQUEST)),
        Some(COMMAND)
    );
}

#[test]
fn response_transfer_retires_an_upload_whose_proof_was_lost() {
    let mut requester = active_engine::<TestStorageLayout>();
    let _ = start(&mut requester);
    requester.outgoing_resources.state_mut(0).retries_left = 0;
    requester
        .outgoing_resources
        .set_timeout_at(0, Some(InstantMillis(30)));
    let mut responder = active_engine::<TestStorageLayout>();
    let response = [0x42; 40];
    let advertisement = advertise_response_segment_from(
        &mut responder,
        CommandId(77),
        RequestId::of_request_data(REQUEST),
        &response,
        None,
        ResourceSegment::whole(response.len() as u64),
        15,
    );
    let pull = feed(&mut requester, &advertisement, 20);
    assert_eq!(
        (
            pull.settlements,
            requester.outgoing_resources.len(),
            requester.receipts.earliest_timeout_at()
        ),
        (vec![], 0, None)
    );
    assert_eq!(pull.frames.len(), 1);
    let mut settled = Vec::new();
    requester.fire_due_resource_deadlines(InstantMillis(30), &mut |b| b.fill(0x72), &mut |r| {
        record(r, &mut settled)
    });
    assert_eq!(settled, []);
    let parts = feed(&mut responder, &pull.frames[0].1, 35);
    assert_eq!(parts.frames.len(), 1);
    let received = feed(&mut requester, &parts.frames[0].1, 40);
    assert_eq!(
        received.responses,
        [(
            COMMAND,
            RequestId::of_request_data(REQUEST),
            response.to_vec()
        )]
    );
    assert_eq!(
        received.settlements,
        [(
            COMMAND,
            Settlement::SendRequest(Ok(PacketReceiptDelivered {
                rtt: RttMillis::new(30),
                evidence: DeliveryEvidence::Response,
            }))
        )]
    );
    assert_eq!(
        (requester.receipts.len(), requester.incoming_resources.len()),
        (0, 0)
    );
}

#[test]
fn a_transfer_failure_cannot_remove_another_commands_receipt() {
    let mut engine = active_engine::<TestStorageLayout>();
    let _ = start(&mut engine);
    let request = RequestId::of_request_data(REQUEST);
    for (link, command, id) in [
        (LinkId::new([0xFF; 16]), COMMAND, request),
        (link_id(), CommandId(92), request),
        (link_id(), COMMAND, RequestId([0xFF; 16])),
    ] {
        assert_eq!(
            engine.receipts.take_request_for_command(&link, command, id),
            None
        );
        assert_eq!(
            engine.receipts.pending_request_command(&link_id(), request),
            Some(COMMAND)
        );
    }
    assert_eq!(
        engine
            .receipts
            .take_request_for_command(&link_id(), COMMAND, request),
        Some(crate::routing::delivery::receipts::ProvenRequestReceipt {
            command_id: COMMAND,
            intent: SendRequestIntent::Application,
            sent_at: InstantMillis(10),
        })
    );
    assert_eq!(
        (engine.receipts.len(), engine.receipts.earliest_timeout_at()),
        (0, None)
    );
}

#[test]
fn request_resources_refuse_segmented_envelopes_before_advertisement() {
    for index in [1, 2] {
        let mut engine = active_engine::<TestStorageLayout>();
        let mut settled = Vec::new();
        engine.ingest_send_resource_segment_into(
            &send(COMMAND),
            ResourceSegment {
                index,
                total_segments: 2,
                total_data_bytes: (REQUEST.len() * 2) as u64,
            },
            InstantMillis(10),
            &mut |b| b.fill(0x71),
            &mut |r: EngineReaction<'_>| record(r, &mut settled),
        );
        assert_eq!(settled, [failed(SendResourceFailure::Sequencing)]);
        assert_eq!(
            (engine.receipts.len(), engine.outgoing_resources.len()),
            (0, 0)
        );
    }
}

#[test]
fn unavailable_build_settles_before_advertisement_and_ignores_duplicate_completion() {
    let mut engine = active_engine::<TestStorageLayout>();
    let mut reservation = None;
    engine.request_resource_build(
        &send(COMMAND),
        ResourceSegment::whole(REQUEST.len() as u64),
        &mut |r| match r {
            EngineReaction::Directive(Directive::Fulfill(
                crate::engine::OwedWork::ResourceBuild(build),
            )) => reservation = Some(build.reservation()),
            _ => unreachable!("build must be reserved"),
        },
    );
    let reservation = reservation.unwrap();
    let mut settled = Vec::new();
    engine.resume_resource_build_unavailable(reservation, &mut |r| record(r, &mut settled));
    assert_eq!(settled, [failed(SendResourceFailure::Rejected(SendResourceRejection::Build(
        crate::routing::links::resources::build_outgoing::BuildOutgoingResourceError::BufferShapeMismatch,
    )))]);
    assert_eq!(
        (engine.receipts.len(), engine.outgoing_resources.len()),
        (0, 0)
    );
    settled.clear();
    engine.resume_resource_build_unavailable(reservation, &mut |r| record(r, &mut settled));
    assert_eq!(settled, []);
}

#[test]
fn accepting_a_response_preserves_an_unrelated_upload() {
    let original = RequestId::of_request_data(REQUEST);
    for (link, command, request) in [
        (LinkId::new([0xFF; 16]), COMMAND, original),
        (link_id(), CommandId(92), original),
        (link_id(), COMMAND, RequestId([0xFF; 16])),
    ] {
        let mut engine = active_engine::<TestStorageLayout>();
        let hash = start(&mut engine);
        assert!(engine
            .receipts
            .take_request_for_command(&link_id(), COMMAND, original)
            .is_some());
        let mut packet_hash = [0; 32];
        packet_hash[..16].copy_from_slice(request.as_bytes());
        assert_eq!(
            engine.receipts.track(OutstandingReceipt {
                packet_hash: PacketHash::new(packet_hash),
                command_id: command,
                kind: ReceiptKind::request(
                    link,
                    ByteLimit::Maximum(256),
                    SendRequestIntent::Application
                ),
                peer_signing_key: IdentitySigningPublicKey::new(crate::crypto::Ed25519PublicKey(
                    [0x99; 32]
                )),
                sent_at: InstantMillis(10),
                timeout_at: InstantMillis(110),
            }),
            None
        );
        engine.claim_resource_response(&link, request);
        assert_eq!(
            (
                engine.outgoing_resources.len(),
                engine.outgoing_resources.hash_at(0),
                engine.outgoing_resources.state(0).command_id,
                engine.receipts.pending_request_command(&link, request),
                engine.receipts.earliest_timeout_at()
            ),
            (1, Some(&hash), COMMAND, Some(command), None)
        );
    }
}
