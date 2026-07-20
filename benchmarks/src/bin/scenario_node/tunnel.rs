use super::link_channel::initiate_single;
use super::*;

pub(super) async fn run_tunnel_probe(manifest: &Manifest, addr: &str, duration: Duration) {
    let mut engine = EngineState::<NodeStorage>::new(generate_identity_secret());
    let node = engine.held_identity_hashes()[0];
    let _destination = engine
        .register_single_destination(
            &node,
            "bench",
            &[&manifest.name],
            b"",
            ProofStrategy::ProveAll,
            LinkRequestPolicy::AcceptAll,
            RatchetPolicy::NoRatchets,
        )
        .expect("registers the bench destination");
    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (in_tx, in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (out_tx, out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let seam = TokioInterfaceSeam::new(TCP_INTERFACE_ID, in_tx, notify_tx, out_rx);
    let egress = Egress::new(vec![(TCP_INTERFACE_ID, out_tx)]);
    let interfaces = vec![tcp::descriptor(
        TCP_INTERFACE_ID,
        tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
    )];
    let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
    let journal = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { observation, .. } => {
            let _ = event_tx.send(Event::Heard(observation.destination));
        }
        Journaled::CommandSettled { id, settlement } => {
            let _ = event_tx.send(Event::Settled(id, settlement));
        }
        _ => {}
    };
    let interface = TcpClientInterface::new_with_id(
        TCP_INTERFACE_ID,
        addr.to_string(),
        tcp::TCP_BITRATE_ESTIMATE,
        ReconnectPolicy::STANDARD,
    );
    tokio::spawn(interface.run(seam));
    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: vec![],
            notify: notify_rx,
            inbound_lanes: vec![(TCP_INTERFACE_ID, in_rx)],
            commands: command_rx,
            egress,
        },
        journal,
    ));
    println!("READY role=initiator");
    initiate_single(&manifest.profile, duration, command_tx, event_rx).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
}

pub(super) async fn tunnel_relay_node(manifest: &Manifest) {
    let engine = EngineState::<NodeStorage>::new(generate_identity_secret());
    let reconnect_at = Duration::from_millis(manifest.profile.reconnect_at_ms);

    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (in_b_tx, in_b_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (out_b_tx, out_b_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let seam_b = TokioInterfaceSeam::new(
        RELAY_SECOND_INTERFACE_ID,
        in_b_tx,
        notify_tx.clone(),
        out_b_rx,
    );
    let egress = Egress::new(vec![(RELAY_SECOND_INTERFACE_ID, out_b_tx)]);
    let interfaces = vec![tcp::descriptor(
        RELAY_SECOND_INTERFACE_ID,
        tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
    )];

    let client_side = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds the client side");
    let addr_a = client_side.local_addr().expect("bound address");
    let peer_side = BenchTcpListener::bind_with_id(
        RELAY_SECOND_INTERFACE_ID,
        "127.0.0.1:0",
        tcp::TCP_BITRATE_ESTIMATE,
    )
    .await
    .expect("binds the peer side");
    let addr_b = peer_side.local_addr().expect("bound address");

    tokio::spawn(peer_side.run(seam_b));
    tokio::spawn(tunnel_client_side(
        client_side,
        command_tx.clone(),
        notify_tx,
        reconnect_at,
    ));
    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: vec![],
            notify: notify_rx,
            inbound_lanes: vec![(RELAY_SECOND_INTERFACE_ID, in_b_rx)],
            commands: command_rx,
            egress,
        },
        |_: Journaled<'_>| {},
    ));
    println!("READY role=relay addr={addr_a}>{addr_b}");
    std::future::pending::<()>().await;
}

async fn tunnel_client_side(
    listener: tokio::net::TcpListener,
    commands: mpsc::UnboundedSender<HostCommand>,
    notify_tx: mpsc::UnboundedSender<InterfaceId>,
    reconnect_at: Duration,
) {
    let mut connection_index = 0u32;
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            return;
        };
        tune(&stream);
        let tag = format!("{peer}#{connection_index}").into_bytes();
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, &tag);
        let descriptor = tcp::descriptor(
            id,
            tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
        );
        let (in_tx, in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
        let (out_tx, out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
        let seam = TokioInterfaceSeam::new(id, in_tx, notify_tx.clone(), out_rx);
        if commands
            .send(HostCommand::AddInterface(AddInterfaceCommand {
                descriptor,
                logical_interface: id,
                inbound: in_rx,
                egress: out_tx,
                connection: None,
                ifac: None,
            }))
            .is_err()
        {
            return;
        }
        let connection =
            TcpServerConnection::new(tag, stream, tcp::TCP_BITRATE_ESTIMATE).run(seam);
        let task = tokio::spawn(connection);
        if connection_index == 0 {
            tokio::spawn(async move {
                tokio::time::sleep(reconnect_at).await;
                task.abort();
            });
        }
        connection_index += 1;
    }
}
