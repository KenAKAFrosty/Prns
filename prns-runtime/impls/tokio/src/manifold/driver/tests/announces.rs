use super::*;

#[tokio::test]
async fn a_commanded_announce_fans_to_every_interface_and_settles() {
    use crate::engine::{
        AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, PrnsCommand, RatchetPolicy,
        Settlement,
    };
    use crate::identity::Zeroizing;
    use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};

    let mut secret = [0u8; 64];
    secret[..32].fill(0x22);
    secret[32..].fill(0x11);
    let mut engine = EngineState::<TestStorageLayout>::new(Zeroizing::new(secret));
    let node = engine.held_identity_hashes()[0];
    let destination = engine
        .register_single_destination(
            &node,
            "personal",
            &["node"],
            b"",
            ProofStrategy::ProveNone,
            LinkRequestPolicy::AcceptAll,
            RatchetPolicy::NoRatchets,
        )
        .expect("registers the single destination");

    let first = InterfaceId::new([0xA1; 8]);
    let second = InterfaceId::new([0xB2; 8]);
    let interfaces = std::vec![descriptor(first), descriptor(second)];

    let (_notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (first_out_tx, mut first_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (second_out_tx, mut second_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let egress = Egress::new(std::vec![(first, first_out_tx), (second, second_out_tx)]);

    let (settled_tx, mut settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
    let app = move |journaled: Journaled<'_>| match journaled {
        Journaled::CommandSettled { id, settlement } => {
            let _ = settled_tx.send((id, settlement));
        }
        Journaled::AnnounceHeard { .. }
        | Journaled::SelfRatchetRotated { .. }
        | Journaled::Delivered(_)
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
        | Journaled::RemoteControlTargetPairingControllerCommitted { .. }
        | Journaled::RemoteControlTargetPairingAuthorizationRequired { .. }
        | Journaled::RemoteControlControllerPairingExpired { .. }
        | Journaled::RemoteControlControllerPairingLinkClosed { .. }
        | Journaled::RemoteControlTargetPairingExpired { .. }
        | Journaled::RemoteControlTargetPairingLinkClosed { .. }
        | Journaled::RemoteControlTargetPairingCompletionRetentionExpired { .. }
        | Journaled::RemoteControlTargetPairingCompletionLinkClosed { .. }
        | Journaled::RemoteControlPairingExpiryFailed { .. } => {}
    };

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ManifoldWiring {
            interfaces,
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![],
            commands: command_rx,
            egress,
        },
        app,
    ));

    command_tx
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(7),
            command: PrnsCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        }))
        .expect("the manifold task holds the receiver");

    let (settled_id, settlement) = tokio::time::timeout(Duration::from_secs(2), settled_rx.recv())
        .await
        .expect("the command settles within the window")
        .expect("the manifold task is alive");
    assert_eq!(settled_id, CommandId(7));
    assert_eq!(settlement, Settlement::AnnounceNow(Ok(())));

    for out_rx in [&mut first_out_rx, &mut second_out_rx] {
        let frame = tokio::time::timeout(Duration::from_secs(2), out_rx.peek())
            .await
            .expect("an announce fires on each interface");
        let (header, _) = WirePacketHeader::parse(frame.frame()).expect("valid announce wire");
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(DestinationHash::from_address(header.address), destination);
    }

    #[cfg(feature = "runtime-metrics")]
    {
        let (reply, snapshot) = oneshot::channel();
        command_tx
            .send(HostCommand::SnapshotMetrics { reply })
            .expect("the manifold task holds the receiver");
        let snapshot = snapshot.await.expect("the manifold returns its metrics");
        assert_eq!(
            snapshot
                .engine
                .announces
                .commands
                .get(crate::engine::AnnounceCommandOutcome::Succeeded),
            1
        );
        assert_eq!(
            snapshot
                .egress
                .announces
                .outcomes
                .get(AnnounceOrigin::Local, AnnounceEgressOutcome::Enqueued),
            2
        );
    }
}
