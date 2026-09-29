use super::*;
use personal_rns::interfaces::bluetooth_auto::{Control, DiscoveryGroupSet, PeerDiscoveryGroups};

pub(super) fn reconnect(
    runner: &mut ManualTaskRunner<'_, Completion>,
    lab: &VirtualBleLab,
    capture: &BleWireCapture,
    handles: &[PrnsNodeHandle],
) {
    let before = capture.snapshot();
    assert_eq!(
        lab.set_reachability(
            BleAddress::new([0; 6]),
            BleAddress::new([1; 6]),
            Reachability::Isolated
        ),
        Ok(TopologyMutation::Applied)
    );
    assert_eq!(lab.active_connection_count(), 0);
    assert!(settle(runner).is_empty());
    assert!(handles
        .iter()
        .all(|handle| ble::member_inventory(handle).is_empty()));
    assert_eq!(
        capture.snapshot(),
        before,
        "retired connections cannot accept more bytes"
    );
    assert_eq!(
        lab.set_reachability(
            BleAddress::new([0; 6]),
            BleAddress::new([1; 6]),
            Reachability::Reachable
        ),
        Ok(TopologyMutation::Applied)
    );
    ble::advance_until(runner, || {
        lab.active_connection_count() == 1
            && handles.iter().enumerate().all(|(index, handle)| {
                ble::member_inventory(handle)
                    == [(
                        InterfaceId::from_channel_tag(
                            InterfaceKind::BluetoothPeer,
                            &[(index ^ 1) as u8; 16],
                        ),
                        ConnectionState::Connected,
                    )]
            })
    });
}

pub(super) fn assert_distinct_incarnations(snapshot: &BleWireSnapshot, boundary: usize) {
    let old = &snapshot.values[..boundary];
    let new = &snapshot.values[boundary..];
    assert!(!old.is_empty() && !new.is_empty());
    assert_ne!(old[0].connection, new[0].connection);
    for phase in [old, new] {
        assert!(phase
            .iter()
            .all(|value| value.connection == phase[0].connection));
        for channel in [BleWireChannel::Control, BleWireChannel::Data] {
            assert!(phase.iter().any(|value| value.channel == channel));
        }
        let greetings: Vec<_> = phase
            .iter()
            .filter(|value| value.channel == BleWireChannel::Control)
            .collect();
        assert_eq!(greetings.len(), 2);
        assert_eq!(
            (greetings[0].from, greetings[0].to),
            (greetings[1].to, greetings[1].from)
        );
        for (ordinal, value) in greetings.into_iter().enumerate() {
            let index = if value.from == BleAddress::new([0; 6]) {
                0
            } else {
                1
            };
            let identity = BleIdentity::new([index; 16]);
            let endpoint = if index == 0 {
                Endpoint::CoreBluetooth(AppleHost::MacOs)
            } else {
                Endpoint::BlueZ(BlueZHost::Linux)
            };
            let capabilities = LinkCapabilities {
                l2cap: None,
                link_mtu: BLE_HW_MTU as u16,
            };
            let discovery_groups =
                PeerDiscoveryGroups::Explicit(DiscoveryGroupSet::default().hashes());
            let expected = match ordinal {
                0 => Control::Hello {
                    identity,
                    endpoint,
                    capabilities,
                    peer_rssi: Some(-40),
                    discovery_groups,
                },
                1 => Control::Welcome {
                    identity,
                    endpoint,
                    capabilities,
                    peer_rssi: Some(-40),
                    discovery_groups,
                },
                _ => unreachable!("exactly one hello and welcome"),
            };
            let mut bytes = [0; CONTROL_MAX_LEN];
            let len = expected
                .encode(&mut bytes)
                .unwrap_or_else(|| unreachable!("bounded greeting"));
            assert_eq!(value.bytes, bytes[..len]);
        }
        let directions: std::collections::BTreeSet<_> =
            phase.iter().map(|value| (value.from, value.to)).collect();
        assert_eq!(
            directions,
            [
                (BleAddress::new([0; 6]), BleAddress::new([1; 6])),
                (BleAddress::new([1; 6]), BleAddress::new([0; 6]))
            ]
            .into_iter()
            .collect()
        );
    }
}

#[test]
fn partition_reconnect_replays_the_entire_transcript_with_explicit_arbitration() {
    let expected = replay(11, 42, Lifecycle::Reconnect);
    let data = |transcript: &Transcript| {
        transcript
            .wire
            .values
            .iter()
            .filter(|value| value.channel == BleWireChannel::Data)
            .cloned()
            .collect::<Vec<_>>()
    };
    for _ in 0..8 {
        let actual = replay(11, 42, Lifecycle::Reconnect);
        actual.assert_replays(&expected);
    }
    let changed = replay(21, 42, Lifecycle::Reconnect);
    assert_eq!(changed.response, expected.response);
    assert_ne!(data(&changed), data(&expected));
}

#[test]
fn every_initial_event_order_repeats_its_own_full_reconnect_transcript() {
    for driver_first in [
        personal_rns::runtime::InterfaceEventSource::Message,
        personal_rns::runtime::InterfaceEventSource::Completion,
    ] {
        for first in selection::FirstEvent::ALL {
            let expected = replay_with_selection(11, 42, Lifecycle::Reconnect, first, driver_first);
            for _ in 0..8 {
                replay_with_selection(11, 42, Lifecycle::Reconnect, first, driver_first)
                    .assert_replays(&expected);
            }
        }
    }
}
