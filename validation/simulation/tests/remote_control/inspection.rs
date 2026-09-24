use super::fixture::*;
use personal_rns::remote_control::*;
use prns_simulation::{FaultPlan, ManualTaskScheduling};

#[test]
fn authenticated_control_exchanges_replay_complete_packets() {
    let run = || {
        with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
            let link = lab.link(CONTROLLER);
            let build = lab.exchange(CONTROLLER, link, RemoteControlRequest::DescribeBuild);
            assert_eq!(
                build,
                RemoteControlResponse::DescribeBuild(
                    RemoteControlBuildVersion::from_text("simulation-v1").expect("fixture build")
                )
            );
            let payload = vec![1; REMOTE_CONTROL_APP_MESSAGE_CAP];
            let reply = lab.exchange(
                CONTROLLER,
                link,
                RemoteControlRequest::AppMessage(
                    RemoteControlAppMessage::from_slice(&payload).expect("max payload"),
                ),
            );
            assert_eq!(
                reply,
                RemoteControlResponse::AppMessage(
                    RemoteControlAppMessage::from_slice(&payload).expect("max payload")
                )
            );
            assert_eq!(
                *lab.calls.borrow(),
                [AppInvocation {
                    controller: lab.nodes[CONTROLLER].identity.identity_hash(),
                    payload
                }]
            );
            (build, reply)
        })
    };
    let first = run();
    assert_eq!(first, run());
    assert_eq!(first, run());
}

#[test]
fn authenticated_parser_errors_and_app_rejection_never_deliver_invalid_payloads() {
    with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
        let link = lab.link(CONTROLLER);
        for (bytes, expected) in [
            (vec![], RemoteControlProtocolError::MalformedRequest),
            (
                vec![0xff, RemoteControlRequestKind::Describe as u8],
                RemoteControlProtocolError::UnsupportedVersion { found: 0xff },
            ),
            (
                vec![1, 0xff],
                RemoteControlProtocolError::UnknownRequestKind { found: 0xff },
            ),
            (
                super::authorization::oversized_app_message(),
                RemoteControlProtocolError::MalformedRequest,
            ),
        ] {
            let task = lab.request(CONTROLLER, link, bytes);
            let mut completed = lab.settle();
            assert_eq!(completed.len(), 1);
            let (found, Event::Response(result)) = completed.remove(0) else {
                unreachable!("parser response");
            };
            assert_eq!(found, task);
            assert_eq!(
                RemoteControlResponse::parse(&result.expect("authenticated error")),
                Ok(RemoteControlResponse::ProtocolError(expected))
            );
            assert!(lab.calls.borrow().is_empty());
        }
        let rejected = RemoteControlAppMessage::from_slice(&[0xff]).expect("bounded payload");
        assert_eq!(
            lab.exchange(CONTROLLER, link, RemoteControlRequest::AppMessage(rejected)),
            RemoteControlResponse::ProtocolError(RemoteControlProtocolError::ApplyFailed {
                request: RemoteControlRequestKind::AppMessage
            })
        );
        let empty = RemoteControlAppMessage::from_slice(&[]).expect("empty is valid");
        assert_eq!(
            lab.exchange(
                CONTROLLER,
                link,
                RemoteControlRequest::AppMessage(empty.clone())
            ),
            RemoteControlResponse::AppMessage(empty)
        );
        assert_eq!(
            *lab.calls.borrow(),
            [
                AppInvocation {
                    controller: lab.nodes[CONTROLLER].identity.identity_hash(),
                    payload: vec![0xff]
                },
                AppInvocation {
                    controller: lab.nodes[CONTROLLER].identity.identity_hash(),
                    payload: vec![]
                },
            ]
        );
    });
}

#[test]
fn snapshots_page_real_runtime_interfaces_and_peers_without_inventing_measurements() {
    use super::fixture::inventory::{IdleInterface, PeerFleet};
    use personal_rns::interfaces::{Membership, RadioIndication, WifiIndication};
    with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
        let link = lab.link(CONTROLLER);
        for index in 0u8..8 {
            lab.nodes[TARGET].handle.add_interface(IdleInterface {
                tag: vec![index, 0x77],
            });
        }
        let fleet = lab.nodes[TARGET].handle.supervise(PeerFleet { members: 9 });
        assert!(lab.settle().is_empty());
        let snapshots = lab.nodes[TARGET].handle.interfaces();
        let mut expected: Vec<_> = snapshots
            .iter()
            .filter(|snapshot| snapshot.membership == Membership::Independent)
            .map(|snapshot| snapshot.id)
            .collect();
        expected.sort_by_key(|id| *id.as_bytes());
        let mut ids = Vec::new();
        let mut page = RemoteControlInterfacePage::First;
        for _ in 0..3 {
            let RemoteControlResponse::InventoryInterfaces(inventory) = lab.exchange(
                CONTROLLER,
                link,
                RemoteControlRequest::InventoryInterfaces { page },
            ) else {
                unreachable!("interface inventory");
            };
            assert!(inventory.entries().len() <= REMOTE_CONTROL_INTERFACE_INVENTORY_CAP);
            ids.extend(inventory.entries().iter().map(|entry| entry.id));
            match inventory.continuation() {
                RemoteControlInterfaceContinuation::More(cursor) => {
                    page = RemoteControlInterfacePage::After(cursor)
                }
                RemoteControlInterfaceContinuation::Complete => break,
            }
        }
        assert_eq!(ids, expected);
        let mut peers = Vec::new();
        let mut page = RemoteControlPeerPage::First;
        for _ in 0..3 {
            let RemoteControlResponse::InventoryInterfacePeers(
                RemoteControlInterfacePeersOutcome::Page(inventory),
            ) = lab.exchange(
                CONTROLLER,
                link,
                RemoteControlRequest::InventoryInterfacePeers {
                    id: fleet.id(),
                    page,
                },
            )
            else {
                unreachable!("peer inventory");
            };
            peers.extend(inventory.peers.iter().copied());
            match inventory.continuation() {
                RemoteControlPeerContinuation::More(cursor) => {
                    page = RemoteControlPeerPage::After(cursor)
                }
                RemoteControlPeerContinuation::Complete => break,
            }
        }
        let mut expected: Vec<_> = snapshots
            .iter()
            .filter(|snapshot| {
                snapshot.membership
                    == Membership::FleetMember {
                        supervisor_id: fleet.id(),
                    }
            })
            .map(|snapshot| snapshot.id)
            .collect();
        expected.sort_by_key(|id| *id.as_bytes());
        assert_eq!(
            peers.iter().map(|peer| peer.id).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(peers.len(), 9);
        for peer in &peers {
            assert_eq!(
                peer.radio,
                RadioIndication::HaLow(WifiIndication::Unavailable)
            );
            assert_eq!(peer.rate_bytes_per_sec, None);
        }
        let RemoteControlResponse::InventoryInterfaceConfig(
            RemoteControlInterfaceConfigOutcome::Card(card),
        ) = lab.exchange(
            CONTROLLER,
            link,
            RemoteControlRequest::InventoryInterfaceConfig { id: fleet.id() },
        )
        else {
            unreachable!("supervisor card");
        };
        assert!(card.failure.is_empty());
        fleet.teardown();
        assert!(lab.settle().is_empty());
        assert_eq!(
            lab.exchange(
                CONTROLLER,
                link,
                RemoteControlRequest::InventoryInterfaceConfig {
                    id: PeerFleet::id()
                }
            ),
            RemoteControlResponse::InventoryInterfaceConfig(
                RemoteControlInterfaceConfigOutcome::UnknownInterface
            )
        );
    });
}

#[test]
fn configured_but_unimplemented_host_commands_are_not_advertised_or_executed() {
    let mut permissions = requests();
    permissions.insert(RemoteControlRequestKind::DescribePower);
    with_policy(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        ControllerPolicy::Granted(permissions),
        |lab| {
            assert_eq!(
                lab.medium
                    .trace()
                    .events
                    .iter()
                    .filter(|event| matches!(
                        event,
                        prns_simulation::MediumEvent::TransmissionAccepted { .. }
                    ))
                    .count(),
                0,
                "startup has no automatic announcement policy"
            );
            let link = lab.link(CONTROLLER);
            let RemoteControlResponse::Describe(description) =
                lab.exchange(CONTROLLER, link, RemoteControlRequest::Describe)
            else {
                unreachable!("description");
            };
            assert_eq!(*description.available_requests(), requests());
            let before = lab.target_response_count();
            let task = lab.request(
                CONTROLLER,
                link,
                encoded(RemoteControlRequest::DescribePower),
            );
            lab.expect_timeout(task);
            assert_eq!(lab.target_response_count(), before);
        },
    );
}

#[test]
fn typed_controller_api_exchanges_every_supported_app_payload_length() {
    with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
        let link = lab.link(CONTROLLER);
        let handle = lab.nodes[CONTROLLER].handle.clone();
        let task = lab.insert(async move {
            for len in 0..=REMOTE_CONTROL_APP_MESSAGE_CAP {
                let payload = RemoteControlAppMessage::from_slice(&vec![len as u8; len])
                    .expect("supported length");
                let (reply, _) = handle
                    .remote_control(link)
                    .app_message(payload.clone())
                    .await
                    .expect("typed app exchange");
                assert_eq!(reply, payload, "payload length {len}");
            }
            Event::Done
        });
        lab.expect_done(task);
        assert_eq!(lab.calls.borrow().len(), REMOTE_CONTROL_APP_MESSAGE_CAP + 1);
    });
}
