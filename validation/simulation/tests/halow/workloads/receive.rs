use super::*;
pub fn receive_pressure(lab: &mut Lab<'_>) {
    lab.announce(TARGET);
    let primary = lab.link(PRIMARY);
    let healthy = lab.link(HEALTHY);
    let id = scope(TARGET).peer_id(mac(PRIMARY));
    let before = lab.nodes[TARGET]
        .handle
        .interfaces()
        .iter()
        .find(|row| row.id == id)
        .expect("known primary")
        .rx_bytes;
    let mut buffer = [0; 1500];
    let envelope = personal_rns::interfaces::wifi_halow::encode(&[0], &mut buffer)
        .expect("envelope containing an invalid RNS packet");
    for _ in 0..128 {
        assert_eq!(
            lab.medium
                .inject(lab.nodes[TARGET].radio, mac(PRIMARY), envelope),
            DeliveryOutcome::Queued
        );
    }
    let competing = personal_rns::remote_control::RemoteControlAppMessage::from_slice(
        b"during-other-peer-flood",
    )
    .expect("bounded competing message");
    let task = lab.request(
        HEALTHY,
        healthy,
        personal_rns::remote_control::RemoteControlRequest::AppMessage(competing.clone()),
    );
    let Event::Response(reply) = lab.wait(task, 1000) else {
        panic!("healthy controller during peer saturation");
    };
    assert_eq!(
        personal_rns::remote_control::RemoteControlResponse::parse(
            &reply.expect("progress with queued flood")
        ),
        Ok(personal_rns::remote_control::RemoteControlResponse::AppMessage(competing))
    );
    let after = lab.nodes[TARGET]
        .handle
        .interfaces()
        .iter()
        .find(|row| row.id == id)
        .expect("surviving primary")
        .rx_bytes;
    assert!(
        after > before && after - before < 128,
        "actual bounded peer lane must drop part of the burst, not silently retain all work"
    );
    lab.measurements.push(Measurement::PeerBurst {
        injected_datagrams: 128,
        delivered_frames: after - before,
    });
    lab.app(HEALTHY, healthy, b"after-other-peer-flood");
    lab.app(PRIMARY, primary, b"flooded-peer-reusable");
    let before = lab.peer_ids(TARGET);
    assert_eq!(
        lab.medium
            .inject(lab.nodes[TARGET].radio, mac(250), b"malformed envelope"),
        DeliveryOutcome::Queued
    );
    assert!(lab.settle().is_empty());
    assert_eq!(lab.peer_ids(TARGET), before);
    for source in 100..118 {
        assert_eq!(
            lab.medium
                .inject(lab.nodes[TARGET].radio, mac(source), envelope),
            DeliveryOutcome::Queued
        );
        assert!(lab.settle().is_empty());
    }
    let peers = lab.peer_ids(TARGET);
    assert_eq!(peers.len(), 16, "production cap");
    assert!(
        before.iter().all(|id| peers.contains(id)),
        "new sources cannot evict live peers"
    );
    assert!(!peers.contains(&scope(TARGET).peer_id(mac(117))));
    lab.app(HEALTHY, healthy, b"at-peer-cap");
    lab.app(PRIMARY, primary, b"at-peer-cap-primary");
    use personal_rns::remote_control::*;
    let id = lab.nodes[TARGET].adapter_id();
    let mut page = RemoteControlPeerPage::First;
    let mut all = Vec::new();
    let mut pages = 0;
    loop {
        assert!(pages < 8, "bounded real peer pagination");
        pages += 1;
        let RemoteControlResponse::InventoryInterfacePeers(
            RemoteControlInterfacePeersOutcome::Page(snapshot),
        ) = lab.exchange(
            HEALTHY,
            healthy,
            RemoteControlRequest::InventoryInterfacePeers { id, page },
        )
        else {
            panic!("actual peer inventory");
        };
        for peer in &snapshot.peers {
            assert_eq!(peer.rate_bytes_per_sec, None);
            assert_eq!(
                peer.radio,
                personal_rns::interfaces::RadioIndication::HaLow(
                    personal_rns::interfaces::WifiIndication::Unavailable
                )
            );
        }
        all.extend(snapshot.peers.iter().map(|peer| peer.id));
        match snapshot.continuation() {
            RemoteControlPeerContinuation::Complete => break,
            RemoteControlPeerContinuation::More(cursor) => {
                page = RemoteControlPeerPage::After(cursor)
            }
        }
    }
    let mut expected = peers;
    expected.push(personal_rns::interfaces::InterfaceId::from_channel_tag(
        personal_rns::interfaces::InterfaceKind::WifiHaLowBroadcast,
        &scope(TARGET).channel_tag(),
    ));
    expected.sort_by_key(|id| *id.as_bytes());
    assert_eq!(
        all, expected,
        "actual peer and shared-channel rows page without repetition"
    );
    assert!(pages > 1);
    wire_contract(lab);
}
#[test]
fn actual_peer_queue_pressure_and_admission_cap_preserve_existing_neighbors() {
    for seed in [0, 1, 42, 0x5eed] {
        let run = || {
            let medium = medium();
            with_lab(medium.clone(), seed, Topology::Shared, receive_pressure);
            medium.snapshot()
        };
        assert_eq!(run(), run());
    }
}
