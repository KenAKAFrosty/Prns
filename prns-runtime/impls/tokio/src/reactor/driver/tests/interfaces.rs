use super::*;

#[tokio::test]
async fn ifac_members_hear_each_other_and_strangers_stay_outside() {
    use crate::interfaces::IfacContext;
    use crate::wire::DestinationHash;

    let source = InterfaceId::new([0xA1; 8]);
    let peer = InterfaceId::new([0xB2; 8]);
    let interfaces = std::vec![descriptor(source), descriptor(peer)];
    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

    let network = || {
        IfacContext::derive(
            Some("testnet"),
            Some("s3cret"),
            crate::interfaces::IfacSize::NARROW,
        )
        .unwrap()
    };
    let ifacs = std::vec![
        InterfaceIfac {
            id: source,
            context: network(),
        },
        InterfaceIfac {
            id: peer,
            context: network(),
        },
    ];

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
    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let app = move |journaled: Journaled<'_>| {
        if let Journaled::AnnounceHeard { observation, .. } = journaled {
            let _ = heard_tx.send(observation.destination);
        }
    };

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs,
            notify: notify_rx,
            inbound_lanes: std::vec![(source, source_in_rx), (peer, peer_in_rx)],
            commands: command_rx,
            egress,
        },
        app,
    ));
    tokio::spawn(source_iface.run(source_seam));
    tokio::spawn(peer_iface.run(peer_seam));

    let clean = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    let mut member_wire = std::vec![0u8; MAX_WIRE_FRAME_LEN];
    let masked_len = network().mask_outbound(&clean, &mut member_wire).unwrap();
    source_wire_in_tx
        .send(member_wire[..masked_len].to_vec())
        .expect("the source interface holds its wire");

    let heard = tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
        .await
        .expect("a member's masked announce is heard")
        .expect("the reactor task is alive");
    assert_eq!(
        heard.as_bytes(),
        bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314").as_slice(),
    );

    let rebroadcast = tokio::time::timeout(Duration::from_secs(2), peer_wire_out_rx.recv())
        .await
        .expect("the rebroadcast leaves through the peer")
        .expect("the peer task is alive");
    assert_eq!(
        rebroadcast[0] & 0x80,
        0x80,
        "the peer's wire only ever carries flagged, masked frames",
    );
    let mut recovered = std::vec![0u8; MAX_WIRE_FRAME_LEN];
    let clean_len = network()
        .unmask_inbound(&rebroadcast, &mut recovered)
        .expect("a member can open the rebroadcast");
    let (header, _) = WirePacketHeader::parse(&recovered[..clean_len]).unwrap();
    assert_eq!(header.packet_type, PacketType::Announce);
    assert_eq!(
        header.hops, 1,
        "the relay bumped the hop count under the mask"
    );

    let stranger = IfacContext::derive(
        Some("testnet"),
        Some("wrong"),
        crate::interfaces::IfacSize::NARROW,
    )
    .unwrap();
    let mut stranger_wire = std::vec![0u8; MAX_WIRE_FRAME_LEN];
    let stranger_len = stranger
        .mask_outbound(
            &bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE),
            &mut stranger_wire,
        )
        .unwrap();
    source_wire_in_tx
        .send(stranger_wire[..stranger_len].to_vec())
        .expect("the source interface holds its wire");
    assert!(
        tokio::time::timeout(Duration::from_secs(1), heard_rx.recv())
            .await
            .is_err(),
        "a stranger's code opens nothing",
    );
}

#[tokio::test]
async fn dynamic_ifac_state_arrives_and_leaves_with_its_interface() {
    use crate::interfaces::{IfacContext, IfacSize};
    use crate::wire::DestinationHash;

    let source = InterfaceId::new([0xD4; 8]);
    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);
    let network = IfacContext::derive(Some("testnet"), Some("s3cret"), IfacSize::NARROW).unwrap();

    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let app = move |journaled: Journaled<'_>| {
        if let Journaled::AnnounceHeard { observation, .. } = journaled {
            let _ = heard_tx.send(observation.destination);
        }
    };

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces: std::vec![],
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![],
            commands: command_rx,
            egress: Egress::new(std::vec![]),
        },
        app,
    ));

    let (mut protected_in, protected_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (protected_out, _protected_wire) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    command_tx
        .send(HostCommand::AddInterface(AddInterfaceCommand {
            descriptor: descriptor(source),
            logical_interface: source,
            inbound: protected_rx,
            egress: protected_out,
            connection: None,
            ifac: Some(network.clone()),
        }))
        .unwrap();
    tokio::task::yield_now().await;

    let clean = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    let mut masked = std::vec![0u8; MAX_WIRE_FRAME_LEN];
    let masked_len = network.mask_outbound(&clean, &mut masked).unwrap();
    protected_in
        .try_grant()
        .unwrap()
        .fill(&masked[..masked_len]);
    protected_in.commit();
    notify_tx.send(source).unwrap();
    tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
        .await
        .unwrap()
        .unwrap();

    command_tx
        .send(HostCommand::RemoveInterface {
            id: source,
            departure: Departure::MayReturn,
        })
        .unwrap();
    let (mut open_in, open_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (open_out, _open_wire) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    command_tx
        .send(HostCommand::AddInterface(AddInterfaceCommand {
            descriptor: descriptor(source),
            logical_interface: source,
            inbound: open_rx,
            egress: open_out,
            connection: None,
            ifac: None,
        }))
        .unwrap();
    tokio::task::yield_now().await;

    let open = bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE);
    open_in.try_grant().unwrap().fill(&open);
    open_in.commit();
    notify_tx.send(source).unwrap();
    tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
        .await
        .unwrap()
        .unwrap();
}
