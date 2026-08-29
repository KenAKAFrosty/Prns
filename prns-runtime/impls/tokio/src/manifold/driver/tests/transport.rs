use super::*;

#[tokio::test]
async fn a_loopback_frame_crosses_the_seam_and_the_rebroadcast_leaves_through_the_peer() {
    let source = InterfaceId::new([0xA1; 8]);
    let peer = InterfaceId::new([0xB2; 8]);
    let interfaces = std::vec![descriptor(source), descriptor(peer)];

    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (peer_in_tx, peer_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);

    let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_wire_out_tx, _source_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_out_tx, source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let source_iface = LoopbackInterface {
        descriptor: descriptor(source),
        wire_in: source_wire_in_rx,
        wire_out: source_wire_out_tx,
    };
    let source_seam =
        TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), source_out_rx);

    let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_wire_out_tx, mut peer_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_out_tx, peer_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let peer_iface = LoopbackInterface {
        descriptor: descriptor(peer),
        wire_in: peer_wire_in_rx,
        wire_out: peer_wire_out_tx,
    };
    let peer_seam = TokioInterfaceSeam::new(peer, peer_in_tx, notify_tx.clone(), peer_out_rx);

    drop(notify_tx);

    let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);

    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<()>();
    let app = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { .. } => {
            let _ = heard_tx.send(());
        }
        Journaled::Delivered(_)
        | Journaled::SelfRatchetRotated { .. }
        | Journaled::CommandSettled { .. }
        | Journaled::AnnounceHeldDropped { .. }
        | Journaled::RouteRemoved { .. }
        | Journaled::LinkEstablished(_)
        | Journaled::PeerIdentified { .. }
        | Journaled::RequestReceived { .. }
        | Journaled::ResponseReceived { .. }
        | Journaled::ResponseSegmentReceived { .. }
        | Journaled::ChannelMessageReceived { .. }
        | Journaled::LinkClosed { .. }
        | Journaled::ResourceReceived { .. }
        | Journaled::ResourceFailed { .. }
        | Journaled::ResourceNeedsDecompression { .. }
        | Journaled::ResourceSegmentReceived { .. }
        | Journaled::ResourceAssembled { .. }
        | Journaled::PersistenceFlushed { .. }
        | Journaled::PersistenceFlushFailed { .. }
        | Journaled::LinkInterfaceMismatch { .. }
        | Journaled::RemoteControlPairingAvailabilityObserved(_)
        | Journaled::RemoteControlPairingExpired { .. }
        | Journaled::RemoteControlTargetPairingConfirmationRequired(_)
        | Journaled::RemoteControlTargetPairingExpired { .. }
        | Journaled::RemoteControlTargetPairingLinkClosed { .. }
        | Journaled::RemoteControlPairingExpiryFailed { .. } => {}
    };

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ManifoldWiring {
            interfaces,
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![(source, source_in_rx), (peer, peer_in_rx)],
            commands: command_rx,
            egress,
        },
        app,
    ));
    tokio::spawn(source_iface.run(source_seam));
    tokio::spawn(peer_iface.run(peer_seam));

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        heard_rx.try_recv().is_err(),
        "an idle manifold journals nothing"
    );
    assert!(
        peer_wire_out_rx.try_recv().is_err(),
        "an idle interface transmits nothing"
    );

    let raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
    let original_hops = WirePacketHeader::parse(&raw)
        .expect("valid announce wire")
        .0
        .hops;
    source_wire_in_tx
        .send(raw)
        .expect("the source interface holds its wire");

    tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
        .await
        .expect("the deposited frame journals within the window")
        .expect("the manifold task is alive");

    let bytes = tokio::time::timeout(Duration::from_secs(2), peer_wire_out_rx.recv())
        .await
        .expect("the rebroadcast reaches the peer's wire within the window")
        .expect("the peer interface task is alive");
    let (header, _) = WirePacketHeader::parse(&bytes).expect("valid rebroadcast wire");
    assert_eq!(header.packet_type, PacketType::Announce);
    assert_eq!(
        header.hops,
        original_hops + 1,
        "the rebroadcast bumps the hop count"
    );
}

#[tokio::test(start_paused = true)]
async fn a_capped_link_holds_a_rebroadcast_burst_then_drains_it_over_time() {
    let source = InterfaceId::new([0xA1; 8]);
    let peer = InterfaceId::new([0xB2; 8]);
    let slow_peer = InterfaceDescriptor {
        bitrate: BitrateBps::guess(1_000),
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        ..descriptor(peer)
    };
    let interfaces = std::vec![descriptor(source), slow_peer];

    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (peer_in_tx, peer_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);

    let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_wire_out_tx, _source_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_out_tx, source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let source_iface = LoopbackInterface {
        descriptor: descriptor(source),
        wire_in: source_wire_in_rx,
        wire_out: source_wire_out_tx,
    };
    let source_seam =
        TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), source_out_rx);

    let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_wire_out_tx, mut peer_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_out_tx, peer_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let peer_iface = LoopbackInterface {
        descriptor: descriptor(peer),
        wire_in: peer_wire_in_rx,
        wire_out: peer_wire_out_tx,
    };
    let peer_seam = TokioInterfaceSeam::new(peer, peer_in_tx, notify_tx.clone(), peer_out_rx);

    drop(notify_tx);

    let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);
    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<()>();

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ManifoldWiring {
            interfaces,
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![(source, source_in_rx), (peer, peer_in_rx)],
            commands: command_rx,
            egress,
        },
        move |journaled: Journaled<'_>| {
            if let Journaled::AnnounceHeard { .. } = journaled {
                let _ = heard_tx.send(());
            }
        },
    ));
    tokio::spawn(source_iface.run(source_seam));
    tokio::spawn(peer_iface.run(peer_seam));

    source_wire_in_tx
        .send(bytes_from_hex(RNS_1_4_2_ANNOUNCE))
        .expect("the source interface holds its wire");
    source_wire_in_tx
        .send(bytes_from_hex(RNS_1_4_2_RATCHETED_ANNOUNCE))
        .expect("the source interface holds its wire");

    heard_rx
        .recv()
        .await
        .expect("the first announce reaches the manifold");
    heard_rx
        .recv()
        .await
        .expect("the second announce reaches the manifold");

    let first = tokio::time::timeout(Duration::from_secs(5), peer_wire_out_rx.recv())
        .await
        .expect("the first rebroadcast leaves the idle link within the window")
        .expect("the peer task is alive");
    assert_eq!(
        WirePacketHeader::parse(&first).unwrap().0.packet_type,
        PacketType::Announce
    );

    assert!(
        tokio::time::timeout(Duration::from_secs(5), peer_wire_out_rx.recv())
            .await
            .is_err(),
        "the cap holds the second rebroadcast far short of its spacing window",
    );

    let second = tokio::time::timeout(Duration::from_secs(120), peer_wire_out_rx.recv())
        .await
        .expect("the held rebroadcast drains once the spacing window passes")
        .expect("the peer task is alive");
    assert_eq!(
        WirePacketHeader::parse(&second).unwrap().0.packet_type,
        PacketType::Announce
    );
    assert_ne!(first, second, "the two rebroadcasts are distinct announces");
}

#[tokio::test(start_paused = true)]
async fn the_manifold_re_emits_a_rebroadcast_once_more_then_retires_it() {
    let source = InterfaceId::new([0xA1; 8]);
    let peer = InterfaceId::new([0xB2; 8]);
    let interfaces = std::vec![descriptor(source), descriptor(peer)];

    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (peer_in_tx, peer_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);

    let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_wire_out_tx, _source_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_out_tx, source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let source_iface = LoopbackInterface {
        descriptor: descriptor(source),
        wire_in: source_wire_in_rx,
        wire_out: source_wire_out_tx,
    };
    let source_seam =
        TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), source_out_rx);

    let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_wire_out_tx, mut peer_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_out_tx, peer_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let peer_iface = LoopbackInterface {
        descriptor: descriptor(peer),
        wire_in: peer_wire_in_rx,
        wire_out: peer_wire_out_tx,
    };
    let peer_seam = TokioInterfaceSeam::new(peer, peer_in_tx, notify_tx.clone(), peer_out_rx);

    drop(notify_tx);

    let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);
    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ManifoldWiring {
            interfaces,
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![(source, source_in_rx), (peer, peer_in_rx)],
            commands: command_rx,
            egress,
        },
        |_journaled: Journaled<'_>| {},
    ));
    tokio::spawn(source_iface.run(source_seam));
    tokio::spawn(peer_iface.run(peer_seam));

    source_wire_in_tx
        .send(bytes_from_hex(RNS_1_4_2_ANNOUNCE))
        .expect("the source interface holds its wire");

    let first = tokio::time::timeout(Duration::from_secs(2), peer_wire_out_rx.recv())
        .await
        .expect("the first emission leaves within the jitter window")
        .expect("the peer task is alive");

    assert!(
        tokio::time::timeout(Duration::from_secs(4), peer_wire_out_rx.recv())
            .await
            .is_err(),
        "the second emission waits the full retransmit interval",
    );

    let second = tokio::time::timeout(Duration::from_secs(120), peer_wire_out_rx.recv())
        .await
        .expect("the manifold re-emits once the retransmit interval passes")
        .expect("the peer task is alive");
    assert_eq!(
        first, second,
        "the retransmit re-emits the same pinned announce, byte for byte",
    );

    assert!(
        tokio::time::timeout(Duration::from_secs(120), peer_wire_out_rx.recv())
            .await
            .is_err(),
        "after two emissions the manifold retires the entry",
    );
}

#[tokio::test]
async fn a_delivery_answers_with_a_proof_directive_on_the_arrival_lane() {
    use crate::crypto::X25519SecretKey;
    use crate::engine::RatchetPolicy;
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::{IdentitySigner, RemoteIdentity, Zeroizing};
    use crate::routing::dedup::PacketHash;
    use crate::routing::proof::IMPLICIT_PROOF_WIRE_LEN;
    use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
    use crate::wire::{
        ContextFlag, DestinationType, IfacFlag, PropagationType, WireContext, BROADCAST_MTU,
    };

    let mut secret = [0u8; 64];
    secret[..32].fill(0x22);
    secret[32..].fill(0x11);
    let secret = Zeroizing::new(secret);

    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
    let mut engine = EngineState::<TestStorageLayout>::new(secret);
    let destination = engine
        .register_single_destination(
            &identity.identity_hash(),
            "personal",
            &["node"],
            b"",
            ProofStrategy::ProveAll,
            LinkRequestPolicy::AcceptAll,
            RatchetPolicy::NoRatchets,
        )
        .expect("registers the single destination");

    let remote = RemoteIdentity::from_public_keys(
        identity.encryption_public_key(),
        identity.signing_public_key(),
    );
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        address: destination.to_address(),
        context: WireContext::None,
    };
    let mut wire = [0u8; BROADCAST_MTU];
    let header_len = header.write(&mut wire).expect("writes the header");
    let sealed = remote
        .encrypt(
            &X25519SecretKey::new([0x77; 32]),
            &[0x88; 16],
            b"prove-through-the-stack",
            &mut wire[header_len..],
        )
        .expect("seals the payload");
    let raw = wire[..header_len + sealed].to_vec();
    let packet_hash = PacketHash::of_wire_packet(&raw).expect("hashes the wire packet");

    let mut expected_proof = std::vec::Vec::new();
    expected_proof.push(0x03);
    expected_proof.push(0x00);
    expected_proof.extend_from_slice(packet_hash.proof_destination().as_bytes());
    expected_proof.push(0x00);
    expected_proof.extend_from_slice(&identity.sign(packet_hash.as_bytes()).0);
    assert_eq!(expected_proof.len(), IMPLICIT_PROOF_WIRE_LEN);

    let source = InterfaceId::new([0xA1; 8]);
    let interfaces = std::vec![descriptor(source)];

    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (mut source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (source_out_tx, mut source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let egress = Egress::new(std::vec![(source, source_out_tx)]);

    let (delivered_tx, mut delivered_rx) = mpsc::unbounded_channel::<()>();
    let app = move |journaled: Journaled<'_>| match journaled {
        Journaled::Delivered(_) => {
            let _ = delivered_tx.send(());
        }
        Journaled::AnnounceHeard { .. }
        | Journaled::SelfRatchetRotated { .. }
        | Journaled::CommandSettled { .. }
        | Journaled::AnnounceHeldDropped { .. }
        | Journaled::RouteRemoved { .. }
        | Journaled::LinkEstablished(_)
        | Journaled::PeerIdentified { .. }
        | Journaled::RequestReceived { .. }
        | Journaled::ResponseReceived { .. }
        | Journaled::ResponseSegmentReceived { .. }
        | Journaled::ChannelMessageReceived { .. }
        | Journaled::LinkClosed { .. }
        | Journaled::ResourceReceived { .. }
        | Journaled::ResourceFailed { .. }
        | Journaled::ResourceNeedsDecompression { .. }
        | Journaled::ResourceSegmentReceived { .. }
        | Journaled::ResourceAssembled { .. }
        | Journaled::PersistenceFlushed { .. }
        | Journaled::PersistenceFlushFailed { .. }
        | Journaled::LinkInterfaceMismatch { .. }
        | Journaled::RemoteControlPairingAvailabilityObserved(_)
        | Journaled::RemoteControlPairingExpired { .. }
        | Journaled::RemoteControlTargetPairingConfirmationRequired(_)
        | Journaled::RemoteControlTargetPairingExpired { .. }
        | Journaled::RemoteControlTargetPairingLinkClosed { .. }
        | Journaled::RemoteControlPairingExpiryFailed { .. } => {}
    };

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ManifoldWiring {
            interfaces,
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![(source, source_in_rx)],
            commands: command_rx,
            egress,
        },
        app,
    ));

    source_in_tx
        .try_grant()
        .expect("an empty lane grants")
        .fill(&raw);
    source_in_tx.commit();
    notify_tx
        .send(source)
        .expect("the manifold task holds the receiver");

    tokio::time::timeout(Duration::from_secs(2), delivered_rx.recv())
        .await
        .expect("the delivery journals within the window")
        .expect("the manifold task is alive");

    let frame = tokio::time::timeout(Duration::from_secs(2), source_out_rx.peek())
        .await
        .expect("the owed proof is emitted within the window");
    assert_eq!(
        frame.frame(),
        expected_proof,
        "the proof is byte-identical to the RNS 1.4.2 implicit proof, on the arrival lane"
    );
}
