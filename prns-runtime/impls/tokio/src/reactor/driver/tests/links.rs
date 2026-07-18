use super::*;

#[tokio::test]
async fn a_link_establishes_and_carries_data_across_two_live_reactors() {
    use crate::engine::test_support::{personal_node_destination, second_secret_key};
    use crate::engine::{
        AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EstablishLink,
        LinkEstablished, RatchetPolicy, SendToLink, SendToLinkFailure, SendToLinkPayload,
        Settlement,
    };
    use crate::routing::delivery::Delivery;
    use crate::routing::links::LinkId;
    use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};

    let initiator_iface = InterfaceId::new([0xA1; 8]);
    let responder_iface = InterfaceId::new([0xB2; 8]);

    let (a_to_b_tx, a_to_b_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (b_to_a_tx, b_to_a_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();

    let initiator_engine = EngineState::<TestStorageLayout>::new(second_secret_key());
    let (a_notify_tx, a_notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (a_in_tx, a_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (a_out_tx, a_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let a_iface = LoopbackInterface {
        descriptor: descriptor(initiator_iface),
        wire_in: b_to_a_rx,
        wire_out: a_to_b_tx,
    };
    let a_seam = TokioInterfaceSeam::new(initiator_iface, a_in_tx, a_notify_tx, a_out_rx);
    let a_egress = Egress::new(std::vec![(initiator_iface, a_out_tx)]);
    let (a_command_tx, a_command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (a_heard_tx, mut a_heard_rx) = mpsc::unbounded_channel::<()>();
    let (a_settled_tx, mut a_settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
    let (a_delivered_tx, mut a_delivered_rx) =
        mpsc::unbounded_channel::<(LinkId, std::vec::Vec<u8>)>();
    let a_app = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { .. } => {
            let _ = a_heard_tx.send(());
        }
        Journaled::CommandSettled { id, settlement } => {
            let _ = a_settled_tx.send((id, settlement));
        }
        Journaled::Delivered(Delivery::Link(link)) => {
            let _ = a_delivered_tx.send((link.link_id, link.plaintext.to_vec()));
        }
        _ => {}
    };

    let responder_engine = {
        use crate::engine::test_support::fixed_secret_key;
        let mut engine: EngineState<TestStorageLayout> = EngineState::new(fixed_secret_key());
        let node = engine.held_identity_hashes()[0];
        engine
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                b"hello-personal",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the proving destination");
        engine
    };
    let (b_notify_tx, b_notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (b_in_tx, b_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (b_out_tx, b_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let b_iface = LoopbackInterface {
        descriptor: descriptor(responder_iface),
        wire_in: a_to_b_rx,
        wire_out: b_to_a_tx,
    };
    let b_seam = TokioInterfaceSeam::new(responder_iface, b_in_tx, b_notify_tx, b_out_rx);
    let b_egress = Egress::new(std::vec![(responder_iface, b_out_tx)]);
    let (b_command_tx, b_command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (b_established_tx, mut b_established_rx) = mpsc::unbounded_channel::<LinkEstablished>();
    let (b_settled_tx, mut b_settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
    let (b_delivered_tx, mut b_delivered_rx) =
        mpsc::unbounded_channel::<(LinkId, std::vec::Vec<u8>)>();
    let b_app = move |journaled: Journaled<'_>| match journaled {
        Journaled::LinkEstablished(established) => {
            let _ = b_established_tx.send(established);
        }
        Journaled::CommandSettled { id, settlement } => {
            let _ = b_settled_tx.send((id, settlement));
        }
        Journaled::Delivered(Delivery::Link(link)) => {
            let _ = b_delivered_tx.send((link.link_id, link.plaintext.to_vec()));
        }
        _ => {}
    };

    tokio::spawn(run(
        initiator_engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces: std::vec![descriptor(initiator_iface)],
            ifacs: std::vec![],
            notify: a_notify_rx,
            inbound_lanes: std::vec![(initiator_iface, a_in_rx)],
            commands: a_command_rx,
            egress: a_egress,
        },
        a_app,
    ));
    tokio::spawn(run(
        responder_engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces: std::vec![descriptor(responder_iface)],
            ifacs: std::vec![],
            notify: b_notify_rx,
            inbound_lanes: std::vec![(responder_iface, b_in_rx)],
            commands: b_command_rx,
            egress: b_egress,
        },
        b_app,
    ));
    tokio::spawn(a_iface.run(a_seam));
    tokio::spawn(b_iface.run(b_seam));

    b_command_tx
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(1),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination: personal_node_destination(),
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        }))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), a_heard_rx.recv())
        .await
        .expect("the announce crosses the wire")
        .expect("the initiator reactor is alive");

    a_command_tx
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(7),
            command: EngineCommand::EstablishLink(EstablishLink {
                destination: personal_node_destination(),
            }),
        }))
        .unwrap();

    let (settled_id, settlement) =
        tokio::time::timeout(Duration::from_secs(5), a_settled_rx.recv())
            .await
            .expect("the link settles within the window")
            .expect("the initiator reactor is alive");
    assert_eq!(settled_id, CommandId(7));
    let Settlement::EstablishLink(Ok(established)) = settlement else {
        panic!("the command must settle established, got {settlement:?}");
    };

    let responder_side = tokio::time::timeout(Duration::from_secs(5), b_established_rx.recv())
        .await
        .expect("the responder journals the link up")
        .expect("the responder reactor is alive");
    assert_eq!(
        responder_side.link_id, established.link_id,
        "one link, two ends",
    );
    assert!(
        responder_side.rtt_ms >= established.rtt_ms,
        "the responder takes max(measured, reported)",
    );

    a_command_tx
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(8),
            command: EngineCommand::SendToLink(SendToLink {
                link_id: established.link_id,
                payload: SendToLinkPayload::from_slice(b"ping over the live link").unwrap(),
            }),
        }))
        .unwrap();
    let delivered = tokio::time::timeout(Duration::from_secs(5), b_delivered_rx.recv())
        .await
        .expect("the responder journals the delivery")
        .expect("the responder reactor is alive");
    assert_eq!(
        delivered,
        (established.link_id, b"ping over the live link".to_vec()),
    );
    let (sent_id, sent) = tokio::time::timeout(Duration::from_secs(5), a_settled_rx.recv())
        .await
        .expect("the initiator's send settles")
        .expect("the initiator reactor is alive");
    assert_eq!(sent_id, CommandId(8));
    let Settlement::SendToLink(Ok(_delivered_receipt)) = sent else {
        panic!("the ProveAll responder's proof settles the send Delivered, got {sent:?}");
    };

    b_command_tx
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(2),
            command: EngineCommand::SendToLink(SendToLink {
                link_id: established.link_id,
                payload: SendToLinkPayload::from_slice(b"pong right back").unwrap(),
            }),
        }))
        .unwrap();
    let sent = loop {
        let (sent_id, sent) = tokio::time::timeout(Duration::from_secs(5), b_settled_rx.recv())
            .await
            .expect("the responder's send settles")
            .expect("the responder reactor is alive");
        if sent_id == CommandId(2) {
            break sent;
        }
    };
    assert_eq!(
        sent,
        Settlement::SendToLink(Err(SendToLinkFailure::Timeout)),
        "the initiator's side never proves, so the responder's send times out — parity",
    );
    let delivered = tokio::time::timeout(Duration::from_secs(5), a_delivered_rx.recv())
        .await
        .expect("the initiator journals the delivery")
        .expect("the initiator reactor is alive");
    assert_eq!(
        delivered,
        (established.link_id, b"pong right back".to_vec()),
    );
}
