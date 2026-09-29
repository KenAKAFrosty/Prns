use super::*;

pub(super) struct Controls {
    pub bridge: BluetoothAutoStatus,
    pub peer: BluetoothAutoStatus,
}

#[derive(Clone, Copy)]
pub(super) enum Profile {
    AppleBridge,
    BluezBridge,
}

pub(super) fn with_bridge(
    profile: Profile,
    scheduling: prns_simulation::ManualTaskScheduling,
    expected_attachments_per_medium: usize,
    run: impl FnOnce(
        &mut ManualTaskRunner<'_, Completion>,
        &VirtualMedium,
        &VirtualBleLab,
        &mut Controls,
        &mut [fixture::Node],
        InterfaceId,
    ),
) {
    let frames = VirtualMedium::new(
        VirtualMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: nonzero(1),
            },
            2,
            8,
            8,
            32768,
            FaultPlan::none(),
        )
        .unwrap_or_else(|error| unreachable!("frame medium: {error}")),
    );
    let first = frames
        .attach(b"recovery-client")
        .unwrap_or_else(|error| unreachable!("client: {error}"));
    let second = frames
        .attach(b"recovery-bridge")
        .unwrap_or_else(|error| unreachable!("bridge: {error}"));
    let frame_id = first.descriptor().id;
    assert_eq!(
        frames.set_reachability(
            first.endpoint_id(),
            second.endpoint_id(),
            Reachability::Reachable
        ),
        Ok(TopologyMutation::Applied)
    );
    let ble = VirtualBleLab::new(
        BleMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: nonzero(1),
            },
            2,
            8,
            2,
            262144,
        )
        .unwrap_or_else(|error| unreachable!("BLE lab: {error}")),
    );
    let (bridge_profile, peer_profile) = match profile {
        Profile::AppleBridge => (
            Endpoint::CoreBluetooth(AppleHost::MacOs),
            Endpoint::BlueZ(BlueZHost::Linux),
        ),
        Profile::BluezBridge => (
            Endpoint::BlueZ(BlueZHost::Linux),
            Endpoint::CoreBluetooth(AppleHost::MacOs),
        ),
    };
    let bridge = supervisor(&ble, BRIDGE, bridge_profile);
    let peer = supervisor(&ble, BLE_PEER, peer_profile);
    let mut controls = Controls {
        bridge: bridge.status(),
        peer: peer.status(),
    };
    assert_eq!(
        ble.set_reachability(
            BleAddress::new([BRIDGE as u8; 6]),
            BleAddress::new([BLE_PEER as u8; 6]),
            Reachability::Reachable
        ),
        Ok(TopologyMutation::Applied)
    );
    let mut driver = ManualTimeDriver::new(
        ManualMedium::FramesAndBle {
            frames: frames.clone(),
            ble: ble.clone(),
        },
        Duration::from_millis(1),
    )
    .unwrap_or_else(|error| unreachable!("driver: {error}"));
    let mut runner = ManualTaskRunner::new_with_scheduling(&mut driver, nonzero(8), scheduling);
    let mut nodes = [
        boot(&mut runner, 0, Ports::Frames(first)),
        boot(
            &mut runner,
            BRIDGE,
            Ports::Bridge {
                frames: second,
                ble: bridge,
            },
        ),
        boot(&mut runner, BLE_PEER, Ports::Ble(peer)),
    ];
    run(
        &mut runner,
        &frames,
        &ble,
        &mut controls,
        &mut nodes,
        frame_id,
    );
    assert_eq!(frames.now(), ble.now());
    fixture::shutdown(
        &mut runner,
        nodes,
        &frames,
        &ble,
        expected_attachments_per_medium,
    );
}

pub(super) fn converge(
    runner: &mut ManualTaskRunner<'_, Completion>,
    ble: &VirtualBleLab,
    nodes: &[fixture::Node],
) {
    advance_until(runner, || {
        ble.active_connection_count() == 1
            && [BRIDGE, BLE_PEER].into_iter().all(|node| {
                member_inventory(&nodes[node].control.handle)
                    == vec![(
                        InterfaceId::from_channel_tag(
                            InterfaceKind::BluetoothPeer,
                            &[(3 - node) as u8; 16],
                        ),
                        ConnectionState::Connected,
                    )]
            })
    });
    for (index, node) in nodes.iter().enumerate() {
        announce(&node.control, hash(index));
    }
    advance_until(runner, || {
        nodes.iter().enumerate().all(|(index, node)| {
            let mut expected: Vec<_> = (0..3).filter(|&peer| peer != index).map(hash).collect();
            expected.sort_by_key(|hash| *hash.as_bytes());
            *node.heard.borrow() == expected
        })
    });
}
