use super::*;

pub(super) async fn relay_node(manifest: &Manifest) {
    let engine = EngineState::<NodeStorage>::new(generate_identity_secret());
    let _ = manifest;

    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (in_a_tx, in_a_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (out_a_tx, out_a_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (in_b_tx, in_b_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (out_b_tx, out_b_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let seam_a = TokioInterfaceSeam::new(TCP_INTERFACE_ID, in_a_tx, notify_tx.clone(), out_a_rx);
    let seam_b = TokioInterfaceSeam::new(RELAY_SECOND_INTERFACE_ID, in_b_tx, notify_tx, out_b_rx);
    let egress = Egress::new(vec![
        (TCP_INTERFACE_ID, out_a_tx),
        (RELAY_SECOND_INTERFACE_ID, out_b_tx),
    ]);
    let interfaces = vec![
        tcp::descriptor(
            TCP_INTERFACE_ID,
            tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
        ),
        tcp::descriptor(
            RELAY_SECOND_INTERFACE_ID,
            tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
        ),
    ];

    let side_a =
        BenchTcpListener::bind_with_id(TCP_INTERFACE_ID, "127.0.0.1:0", tcp::TCP_BITRATE_ESTIMATE)
            .await
            .expect("binds side a");
    let addr_a = side_a.local_addr().expect("bound address");
    let side_b = BenchTcpListener::bind_with_id(
        RELAY_SECOND_INTERFACE_ID,
        "127.0.0.1:0",
        tcp::TCP_BITRATE_ESTIMATE,
    )
    .await
    .expect("binds side b");
    let addr_b = side_b.local_addr().expect("bound address");
    tokio::spawn(side_a.run(seam_a));
    tokio::spawn(side_b.run(seam_b));
    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: vec![],
            notify: notify_rx,
            inbound_lanes: vec![
                (TCP_INTERFACE_ID, in_a_rx),
                (RELAY_SECOND_INTERFACE_ID, in_b_rx),
            ],
            commands: command_rx,
            egress,
        },
        |_: Journaled<'_>| {},
    ));
    println!("READY role=relay addr={addr_a}>{addr_b}");
    std::future::pending::<()>().await;
}

pub(super) async fn chain_node(upstream: &str) {
    let engine = EngineState::<NodeStorage>::new(generate_identity_secret());

    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (in_down_tx, in_down_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (out_down_tx, out_down_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (in_up_tx, in_up_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (out_up_tx, out_up_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let seam_down =
        TokioInterfaceSeam::new(TCP_INTERFACE_ID, in_down_tx, notify_tx.clone(), out_down_rx);
    let seam_up =
        TokioInterfaceSeam::new(RELAY_SECOND_INTERFACE_ID, in_up_tx, notify_tx, out_up_rx);
    let egress = Egress::new(vec![
        (TCP_INTERFACE_ID, out_down_tx),
        (RELAY_SECOND_INTERFACE_ID, out_up_tx),
    ]);
    let interfaces = vec![
        tcp::descriptor(
            TCP_INTERFACE_ID,
            tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
        ),
        tcp::descriptor(
            RELAY_SECOND_INTERFACE_ID,
            tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
        ),
    ];

    let downstream =
        BenchTcpListener::bind_with_id(TCP_INTERFACE_ID, "127.0.0.1:0", tcp::TCP_BITRATE_ESTIMATE)
            .await
            .expect("binds downstream side");
    let addr = downstream.local_addr().expect("bound address");
    let up = TcpClientInterface::new_with_id(
        RELAY_SECOND_INTERFACE_ID,
        upstream.to_string(),
        tcp::TCP_BITRATE_ESTIMATE,
        ReconnectPolicy::STANDARD,
    );
    tokio::spawn(downstream.run(seam_down));
    tokio::spawn(up.run(seam_up));
    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: vec![],
            notify: notify_rx,
            inbound_lanes: vec![
                (TCP_INTERFACE_ID, in_down_rx),
                (RELAY_SECOND_INTERFACE_ID, in_up_rx),
            ],
            commands: command_rx,
            egress,
        },
        |_: Journaled<'_>| {},
    ));
    println!("READY role=chain addr={addr}");
    std::future::pending::<()>().await;
}
