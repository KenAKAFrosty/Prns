use super::fixture::*;
use inventory::{ConfiguredInterface, IdleInterface, PeerFleet};
use personal_rns::interfaces::{BitrateBps, ConnectionState, InterfaceId, InterfaceKind};
use personal_rns::remote_control::*;
use personal_rns::runtime::StreamId;
use prns_simulation::{FaultPlan, ManualTaskScheduling};
use std::sync::{Arc, Mutex};

fn interfaces(lab: &mut Lab<'_>, link: personal_rns::routing::links::LinkId) -> Vec<InterfaceId> {
    let mut page = RemoteControlInterfacePage::First;
    let mut result = Vec::new();
    for _ in 0..8 {
        let RemoteControlResponse::InventoryInterfaces(snapshot) = lab.exchange(
            CONTROLLER,
            link,
            RemoteControlRequest::InventoryInterfaces { page },
        ) else {
            unreachable!("inventory")
        };
        result.extend(snapshot.entries().iter().map(|entry| entry.id));
        match snapshot.continuation() {
            RemoteControlInterfaceContinuation::Complete => return result,
            RemoteControlInterfaceContinuation::More(cursor) => {
                page = RemoteControlInterfacePage::After(cursor)
            }
        }
    }
    unreachable!("bounded pagination")
}

fn peers(lab: &mut Lab<'_>, link: personal_rns::routing::links::LinkId) -> Vec<InterfaceId> {
    let mut page = RemoteControlPeerPage::First;
    let mut result = Vec::new();
    for _ in 0..8 {
        let RemoteControlResponse::InventoryInterfacePeers(
            RemoteControlInterfacePeersOutcome::Page(snapshot),
        ) = lab.exchange(
            CONTROLLER,
            link,
            RemoteControlRequest::InventoryInterfacePeers {
                id: PeerFleet::id(),
                page,
            },
        )
        else {
            unreachable!("peer page")
        };
        result.extend(snapshot.peers.iter().map(|peer| peer.id));
        match snapshot.continuation() {
            RemoteControlPeerContinuation::Complete => return result,
            RemoteControlPeerContinuation::More(cursor) => {
                page = RemoteControlPeerPage::After(cursor)
            }
        }
    }
    unreachable!("bounded peers")
}

#[test]
fn pagination_during_churn_is_live_and_refetch_converges_to_independently_known_membership() {
    let run = || {
        with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
            let link = lab.link(CONTROLLER);
            let mut attached = Vec::new();
            for index in 0u8..9 {
                attached.push(lab.nodes[TARGET].handle.add_interface(IdleInterface {
                    tag: vec![index, 0x99],
                }));
            }
            let fleet = lab.nodes[TARGET].handle.supervise(PeerFleet { members: 9 });
            assert!(lab.settle().is_empty());
            let RemoteControlResponse::InventoryInterfaces(first) = lab.exchange(
                CONTROLLER,
                link,
                RemoteControlRequest::InventoryInterfaces {
                    page: RemoteControlInterfacePage::First,
                },
            ) else {
                unreachable!("first")
            };
            let RemoteControlInterfaceContinuation::More(cursor) = first.continuation() else {
                unreachable!("multipage")
            };
            let RemoteControlResponse::InventoryInterfacePeers(
                RemoteControlInterfacePeersOutcome::Page(first_peers),
            ) = lab.exchange(
                CONTROLLER,
                link,
                RemoteControlRequest::InventoryInterfacePeers {
                    id: fleet.id(),
                    page: RemoteControlPeerPage::First,
                },
            )
            else {
                unreachable!("first peers")
            };
            let RemoteControlPeerContinuation::More(peer_cursor) = first_peers.continuation()
            else {
                unreachable!("multiple peer pages")
            };
            for interface in attached {
                interface.teardown();
            }
            fleet.teardown();
            assert!(lab.settle().is_empty());
            for index in 20u8..29 {
                lab.nodes[TARGET].handle.add_interface(IdleInterface {
                    tag: vec![index, 0x99],
                });
            }
            let replacement = lab.nodes[TARGET].handle.supervise(PeerFleet { members: 2 });
            assert!(lab.settle().is_empty());
            assert!(matches!(
                lab.exchange(
                    CONTROLLER,
                    link,
                    RemoteControlRequest::InventoryInterfaces {
                        page: RemoteControlInterfacePage::After(cursor)
                    }
                ),
                RemoteControlResponse::InventoryInterfaces(_)
            ));
            assert!(matches!(
                lab.exchange(
                    CONTROLLER,
                    link,
                    RemoteControlRequest::InventoryInterfacePeers {
                        id: replacement.id(),
                        page: RemoteControlPeerPage::After(peer_cursor)
                    }
                ),
                RemoteControlResponse::InventoryInterfacePeers(_)
            ));
            let mut expected = vec![PeerFleet::id()];
            expected.extend((20u8..29).map(|index| {
                InterfaceId::from_channel_tag(InterfaceKind::Loopback, &[index, 0x99])
            }));
            expected.sort_by_key(|id| *id.as_bytes());
            assert_eq!(interfaces(lab, link), expected);
            let mut expected_peers: Vec<_> = (0usize..2)
                .map(|index| {
                    InterfaceId::from_channel_tag(
                        InterfaceKind::WifiHaLowPeer,
                        &index.to_be_bytes(),
                    )
                })
                .collect();
            expected_peers.sort_by_key(|id| *id.as_bytes());
            assert_eq!(peers(lab, link), expected_peers);
            replacement.teardown();
            assert!(lab.settle().is_empty());
            (expected, expected_peers)
        })
    };
    assert_eq!(run(), run());
}

#[test]
fn coalesced_watch_invalidation_refetches_configuration_and_peer_state_after_reconnect() {
    with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
        let link = lab.link(CONTROLLER);
        let watch = lab.watch(link, StreamId::new(2).expect("stream"));
        let read = lab.read_watch(watch);
        let (watch, initial) = watch_result(read, lab.settle());
        assert_eq!(
            initial.expect("initial"),
            RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
        );
        let connection = Arc::new(Mutex::new(ConnectionState::Disabled));
        let tag = b"mutable-config".to_vec();
        let id = InterfaceId::from_channel_tag(InterfaceKind::Loopback, &tag);
        let attached = lab.nodes[TARGET].handle.add_interface(ConfiguredInterface {
            tag: tag.clone(),
            bitrate: BitrateBps::guess(1_000_000),
            connection: connection.clone(),
        });
        let fleet = lab.nodes[TARGET].handle.supervise(PeerFleet { members: 7 });
        assert!(lab.settle().is_empty());
        let prior = lab.exchange(
            CONTROLLER,
            link,
            RemoteControlRequest::InventoryInterfaceConfig { id },
        );
        attached.teardown();
        fleet.teardown();
        assert!(lab.settle().is_empty());
        *connection.lock().expect("connection") = ConnectionState::Connected;
        let attached = lab.nodes[TARGET].handle.add_interface(ConfiguredInterface {
            tag,
            bitrate: BitrateBps::guess(2_000_000),
            connection,
        });
        let fleet = lab.nodes[TARGET].handle.supervise(PeerFleet { members: 1 });
        assert!(lab.settle().is_empty());
        let read = lab.read_watch(watch);
        assert!(lab.settle().is_empty());
        let (watch, invalidated) = watch_result(read, lab.advance(500));
        assert_eq!(
            invalidated.expect("coalesced"),
            RemoteControlStreamEvent::ResyncRequired { sequence: 2 }
        );
        let current = lab.exchange(
            CONTROLLER,
            link,
            RemoteControlRequest::InventoryInterfaceConfig { id },
        );
        assert_ne!(
            current, prior,
            "configuration is refetched after invalidation"
        );
        assert_eq!(peers(lab, link).len(), 1);
        assert!(interfaces(lab, link).contains(&id));
        drop(watch);
        assert!(lab.nodes[CONTROLLER].handle.close_link(link));
        assert!(lab.settle().is_empty());
        let fresh = lab.link(CONTROLLER);
        let watch = lab.watch(fresh, StreamId::new(2).expect("same stream on new link"));
        let read = lab.read_watch(watch);
        let (_, resync) = watch_result(read, lab.settle());
        assert_eq!(
            resync.expect("reconnect resync"),
            RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
        );
        assert_eq!(
            lab.exchange(
                CONTROLLER,
                fresh,
                RemoteControlRequest::InventoryInterfaceConfig { id }
            ),
            current
        );
        assert_eq!(peers(lab, fresh).len(), 1);
        attached.teardown();
        fleet.teardown();
        assert!(lab.settle().is_empty());
    });
}

#[test]
fn detected_sequence_gap_triggers_refetch_then_reconnect_starts_a_fresh_sequence() {
    use tokio::io::AsyncWriteExt;
    with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
        let link = lab.link(CONTROLLER);
        let stream = StreamId::new(6).expect("stream ID");
        let watch = lab.watch(link, stream);
        let read = lab.read_watch(watch);
        let (watch, initial) = watch_result(read, lab.settle());
        assert_eq!(
            initial.expect("initial resync"),
            RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
        );
        let attached = lab.nodes[TARGET].handle.add_interface(IdleInterface {
            tag: b"after-gap".to_vec(),
        });
        assert!(lab.settle().is_empty());
        let mut bytes = [0; REMOTE_CONTROL_STREAM_EVENT_LEN];
        RemoteControlStreamEvent::ResyncRequired { sequence: 3 }
            .write_into(&mut bytes)
            .expect("gap frame");
        let mut writer = lab.nodes[TARGET].handle.byte_stream_writer(link, stream);
        let inject = lab.insert(async move {
            writer
                .write_all(&bytes)
                .await
                .expect("authorized test producer");
            Event::Done
        });
        lab.expect_done(inject);
        let read = lab.read_watch(watch);
        let (watch, gap) = watch_result(read, lab.settle());
        assert!(matches!(
            gap,
            Err(
                personal_rns::runtime::RemoteControlWatchReadError::SequenceGap {
                    expected: 2,
                    found: 3
                }
            )
        ));
        assert!(interfaces(lab, link).contains(&attached.id()));
        drop(watch);
        assert!(lab.nodes[CONTROLLER].handle.close_link(link));
        assert!(lab.settle().is_empty());
        let link = lab.link(CONTROLLER);
        let watch = lab.watch(link, stream);
        let read = lab.read_watch(watch);
        let (_, fresh) = watch_result(read, lab.settle());
        assert_eq!(
            fresh.expect("new resync"),
            RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
        );
        assert!(interfaces(lab, link).contains(&attached.id()));
        attached.teardown();
        assert!(lab.settle().is_empty());
    });
}
