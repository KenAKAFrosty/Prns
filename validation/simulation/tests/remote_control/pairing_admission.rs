use super::{durability, fixture::*};
use persistence::Storage;
use personal_rns::engine::{RequestResponseTimeout, SendRequestFailure};
use personal_rns::remote_control::*;
use personal_rns::routing::{links::LinkId, request_handlers::RequestPathHash};
use personal_rns::runtime::{RemoteControlPairingControl, SendError};
use personal_rns::units::DurationMillis;
use personal_rns::wire::{WireContext, WirePacketHeader};
use prns_simulation::*;

fn pairing_link(
    lab: &mut Lab<'_>,
    controller: usize,
    endpoint: RemoteControlPairingEndpoint,
) -> LinkId {
    let handle = lab.nodes[controller].handle.clone();
    let identity = lab.nodes[controller].identity.identity_hash();
    let task = lab.insert(async move {
        let link = handle
            .establish_link(endpoint.destination_hash())
            .await
            .expect("pairing link");
        handle
            .identify(link, identity)
            .await
            .expect("verified link identity");
        Event::Linked(link)
    });
    let mut completed = lab.settle();
    assert_eq!(completed.len(), 1);
    let (found, Event::Linked(link)) = completed.remove(0) else {
        unreachable!("link")
    };
    assert_eq!(found, task);
    assert!(lab.advance(150).is_empty());
    link
}
fn exchange(
    lab: &mut Lab<'_>,
    controller: usize,
    link: LinkId,
    request: &RemoteControlPairingRequest,
) -> Result<RemoteControlPairingResponse, SendError<SendRequestFailure>> {
    let mut bytes = [0; RemoteControlPairingRequest::MAX_ENCODED_LEN];
    let len = request.write_into(&mut bytes).expect("pairing encoding");
    let mut packed = vec![0; len + 5];
    let header =
        personal_rns::routing::links::request::write_packed_binary_header(len, &mut packed)
            .expect("binary envelope");
    packed[header..header + len].copy_from_slice(&bytes[..len]);
    packed.truncate(header + len);
    let bytes = packed;
    let handle = lab.nodes[controller].handle.clone();
    let task = lab.insert(async move {
        Event::Response(
            handle
                .request_with_response_timeout(
                    link,
                    RequestPathHash::of(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID),
                    &bytes,
                    RequestResponseTimeout::Exact(DurationMillis(REQUEST_TIMEOUT_MS)),
                )
                .await
                .map(|(bytes, _)| bytes),
        )
    });
    let mut completed = lab.settle();
    if completed.is_empty() {
        completed = lab.advance(REQUEST_TIMEOUT_MS);
    }
    assert_eq!(completed.len(), 1);
    let (found, Event::Response(reply)) = completed.remove(0) else {
        unreachable!("pairing response")
    };
    assert_eq!(found, task);
    reply.map(|bytes| {
        RemoteControlPairingResponse::parse(
            personal_rns::routing::links::request::parse_packed_binary(&bytes)
                .expect("binary response envelope"),
        )
        .expect("typed pairing response")
    })
}
#[test]
fn identity_mismatch_replayed_begin_and_competing_controller_stay_silent_without_replacing_attempt()
{
    with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        Some(Storage::new()),
        |lab| {
            let opened = lab.open_pairing();
            let outsider = pairing_link(lab, OUTSIDER, opened.endpoint);
            let other = pairing_link(lab, OPERATOR, opened.endpoint);
            let wrong = RemoteControlPairingRequest::Begin(RemoteControlPairingBegin::new(
                lab.nodes[OPERATOR].identity,
                opened.endpoint,
                opened.invitation_code.clone(),
            ));
            let before = lab.target_response_count();
            assert_eq!(
                exchange(lab, OUTSIDER, outsider, &wrong),
                Err(SendError::Failed(SendRequestFailure::Timeout))
            );
            assert_eq!(lab.target_response_count(), before);
            let begin = RemoteControlPairingBegin::new(
                lab.nodes[OUTSIDER].identity,
                opened.endpoint,
                opened.invitation_code.clone(),
            );
            assert!(matches!(
                exchange(
                    lab,
                    OUTSIDER,
                    outsider,
                    &RemoteControlPairingRequest::Begin(begin)
                ),
                Ok(RemoteControlPairingResponse::Offer(_))
            ));
            let confirmation = lab.target_confirmation();
            let replay = RemoteControlPairingRequest::Begin(RemoteControlPairingBegin::new(
                lab.nodes[OUTSIDER].identity,
                opened.endpoint,
                opened.invitation_code.clone(),
            ));
            let before = lab.target_response_count();
            for (controller, link, request) in
                [(OUTSIDER, outsider, &replay), (OPERATOR, other, &wrong)]
            {
                assert_eq!(
                    exchange(lab, controller, link, request),
                    Err(SendError::Failed(SendRequestFailure::Timeout))
                );
            }
            assert_eq!(lab.target_response_count(), before);
            assert!(!lab.pairing.borrow().iter().any(|observation| matches!(
                observation.event,
                pairing::PairingEvent::TargetConfirmation(_)
            )));
            let handle = lab.nodes[TARGET].handle.clone();
            let task = lab.insert(async move {
                handle
                    .reject_remote_control_target_pairing(confirmation.rejection())
                    .await
                    .expect("original attempt remains rejectable");
                Event::Done
            });
            lab.expect_done(task);
        },
    );
}

#[derive(Debug)]
enum Completion {
    Delivered,
    Lost,
}
fn complete(
    lab: &mut Lab<'_>,
    storage: Storage,
    completion: Completion,
) -> RemoteControlPairingResponse {
    let opened = lab.open_pairing();
    let link = pairing_link(lab, OUTSIDER, opened.endpoint);
    let begin = RemoteControlPairingBegin::new(
        lab.nodes[OUTSIDER].identity,
        opened.endpoint,
        opened.invitation_code.clone(),
    );
    let RemoteControlPairingResponse::Offer(offer) = exchange(
        lab,
        OUTSIDER,
        link,
        &RemoteControlPairingRequest::Begin(RemoteControlPairingBegin::new(
            lab.nodes[OUTSIDER].identity,
            opened.endpoint,
            opened.invitation_code,
        )),
    )
    .expect("offer") else {
        unreachable!("offer")
    };
    let transcript = offer
        .verify(
            RemoteControlPairingContext::new(opened.endpoint, link),
            &begin,
        )
        .expect("signed target offer");
    let confirmation = lab.target_confirmation();
    let handle = lab.nodes[TARGET].handle.clone();
    let task = lab.insert(async move {
        handle
            .approve_remote_control_target_pairing(confirmation.approval())
            .await
            .expect("target approval");
        Event::Done
    });
    lab.expect_done(task);
    let commit = RemoteControlPairingRequest::Commit(RemoteControlPairingCommit::new(&transcript));
    let reply = exchange(lab, OUTSIDER, link, &commit);
    match completion {
        Completion::Delivered => assert!(matches!(
            reply,
            Ok(RemoteControlPairingResponse::Completed(_))
        )),
        Completion::Lost => assert_eq!(reply, Err(SendError::Failed(SendRequestFailure::Timeout))),
    }
    let restored = exchange(lab, OUTSIDER, link, &commit).expect("retained exact completion");
    if let Ok(reply) = reply {
        assert_eq!(restored, reply);
    }
    assert_eq!(
        storage
            .grants()
            .iter()
            .filter(|grant| grant.controller() == &lab.nodes[OUTSIDER].identity)
            .count(),
        1
    );
    assert_eq!(
        lab.pairing
            .borrow()
            .iter()
            .filter(|observation| matches!(
                observation.event,
                pairing::PairingEvent::TargetPersisted(_)
            ))
            .count(),
        1
    );
    lab.restart_target(storage.clone());
    let fresh = lab.link(OUTSIDER);
    assert!(matches!(
        lab.exchange(OUTSIDER, fresh, RemoteControlRequest::DescribeBuild),
        RemoteControlResponse::DescribeBuild(_)
    ));
    restored
}
#[test]
fn lost_pairing_completion_replays_exactly_without_reapplying_committed_authority() {
    let storage = Storage::new();
    let (_, baseline) = with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        Some(storage.clone()),
        |lab| complete(lab, storage, Completion::Delivered),
    );
    let ordinal = baseline
        .iter()
        .filter_map(|event| match event {
            MediumEvent::TransmissionAccepted { ordinal, frame, .. }
                if WirePacketHeader::parse(frame)
                    .is_ok_and(|(header, _)| header.context == WireContext::Response) =>
            {
                Some(*ordinal)
            }
            _ => None,
        })
        .nth(1)
        .expect("completion response boundary");
    let plan = FaultPlan::new(vec![TransmissionRule::drop(ordinal)]).expect("one lost completion");
    let run = || {
        let storage = Storage::new();
        with_storage(
            ManualTaskScheduling::Cyclic,
            plan.clone(),
            durability::policy(),
            Some(storage.clone()),
            |lab| complete(lab, storage, Completion::Lost),
        )
    };
    assert_eq!(run(), run());
}
