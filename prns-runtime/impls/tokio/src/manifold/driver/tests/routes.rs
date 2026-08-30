use super::*;

#[tokio::test]
async fn routing_control_drops_a_live_route_and_journals_the_explicit_removal() {
    let source = InterfaceId::new([0xD5; 8]);
    let engine = EngineState::<TestStorageLayout>::default();
    let store = InterfaceStore::new();
    let mut store_changes = store.subscribe();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (mut inbound_tx, inbound_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let handle = PrnsNodeHandle::over(command_tx);
    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let (dropped_tx, mut dropped_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let app = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { observation, .. } => {
            let _ = heard_tx.send(observation.destination);
        }
        Journaled::RouteRemoved {
            destination,
            cause: RouteRemovalCause::Dropped,
        } => {
            let _ = dropped_tx.send(destination);
        }
        Journaled::Delivered(_)
        | Journaled::SelfRatchetRotated { .. }
        | Journaled::AnnounceHeldDropped { .. }
        | Journaled::CommandSettled { .. }
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

    tokio::spawn(run_with_store(
        engine,
        TokioHost::new(),
        ManifoldWiring {
            interfaces: std::vec![descriptor(source)],
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![(source, inbound_rx)],
            commands: command_rx,
            egress: Egress::new(std::vec![]),
        },
        app,
        store.clone(),
        CryptoPoolConfig::Inline,
    ));

    let announce = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
    inbound_tx.try_grant().unwrap().fill(&announce);
    inbound_tx.commit();
    notify_tx.send(source).unwrap();
    let destination = tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), store_changes.changed())
        .await
        .unwrap();
    assert_eq!(store.counts(source).destinations, 1);

    assert_eq!(
        handle.drop_route(destination).await,
        Ok(DropRouteOutcome::Dropped)
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), dropped_rx.recv())
            .await
            .unwrap(),
        Some(destination)
    );
    tokio::time::timeout(Duration::from_secs(2), store_changes.changed())
        .await
        .unwrap();
    assert_eq!(store.counts(source).destinations, 0);
    assert_eq!(
        handle.drop_route(destination).await,
        Ok(DropRouteOutcome::NotFound)
    );
    assert!(dropped_rx.try_recv().is_err());

    #[cfg(feature = "runtime-metrics")]
    assert_eq!(
        handle
            .metrics_snapshot()
            .await
            .unwrap()
            .reliability
            .route_removals
            .get(crate::runtime::RuntimeRouteRemoval::Dropped),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn the_manifold_culls_an_expired_route_at_its_deadline() {
    use crate::engine::{
        CommandId, PrnsCommand, SendSinglePacket, SendSinglePacketFailure, SendSinglePacketPayload,
        SendSinglePacketRejection, Settlement,
    };
    use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;
    use crate::wire::DestinationHash;

    let source = InterfaceId::new([0xA1; 8]);
    let interfaces = std::vec![descriptor(source)];
    let engine = EngineState::<TestStorageLayout>::default();

    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (wire_in_tx, wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (wire_out_tx, _wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (out_tx, out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let iface = LoopbackInterface {
        descriptor: descriptor(source),
        wire_in: wire_in_rx,
        wire_out: wire_out_tx,
    };
    let seam = TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), out_rx);
    drop(notify_tx);
    let egress = Egress::new(std::vec![(source, out_tx)]);
    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();

    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let (expired_tx, mut expired_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let (settled_tx, mut settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
    let app = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { observation, .. } => {
            let _ = heard_tx.send(observation.destination);
        }
        Journaled::RouteRemoved {
            destination,
            cause: RouteRemovalCause::Expired,
        } => {
            let _ = expired_tx.send(destination);
        }
        Journaled::CommandSettled { id, settlement } => {
            let _ = settled_tx.send((id, settlement));
        }
        Journaled::Delivered(_)
        | Journaled::SelfRatchetRotated { .. }
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
            inbound_lanes: std::vec![(source, source_in_rx)],
            commands: command_rx,
            egress,
        },
        app,
    ));
    tokio::spawn(iface.run(seam));

    wire_in_tx
        .send(bytes_from_hex(RNS_1_4_2_ANNOUNCE))
        .expect("the interface holds its wire");
    let destination = tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
        .await
        .expect("the announce is heard, so the route exists before the deadline")
        .expect("the manifold task is alive");

    tokio::time::sleep(Duration::from_millis(DEFAULT_ROUTE_EXPIRY_MILLIS + 10_000)).await;

    let expired = tokio::time::timeout(Duration::from_secs(2), expired_rx.recv())
        .await
        .expect("the cull journals the removal at the expiry deadline")
        .expect("the manifold task is alive");
    assert_eq!(
        expired, destination,
        "the expired route names its destination"
    );

    command_tx
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(3),
            command: PrnsCommand::SendSinglePacket(SendSinglePacket {
                destination,
                payload: SendSinglePacketPayload::from_slice(b"late").expect("fits the MDU"),
            }),
        }))
        .expect("the manifold task holds the receiver");

    let (settled_id, settlement) = tokio::time::timeout(Duration::from_secs(2), settled_rx.recv())
        .await
        .expect("the late send settles")
        .expect("the manifold task is alive");
    assert_eq!(settled_id, CommandId(3));
    assert_eq!(
        settlement,
        Settlement::SendSinglePacket(Err(SendSinglePacketFailure::Rejected(
            SendSinglePacketRejection::NoRouteToDestination
        ))),
        "the manifold woke at the route's expiry and culled it",
    );
}
