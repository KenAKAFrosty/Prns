use super::*;

pub async fn run(spawner: Spawner) {
    let C6Hardware {
        usb_rx,
        usb_tx,
        #[cfg(feature = "esp-now")]
        wifi,
        #[cfg(feature = "bluetooth-auto")]
        bluetooth,
        identity_entropy,
        mac,
        timebase,
        _rtc,
    } = XiaoEsp32C6::bringup();

    let node_bootstrap = crate::identity::bootstrap_node_identity();
    crate::identity::log_persistence("node", node_bootstrap.persistence());
    let ble_bootstrap = crate::identity::bootstrap_ble_identity();
    crate::identity::log_persistence("Bluetooth", ble_bootstrap.persistence());
    drop(identity_entropy);

    #[cfg(feature = "esp-now")]
    let (_espnow_controller, espnow, espnow_status) = {
        let wifi_config = ControllerConfig::default()
            .with_static_rx_buf_num(4)
            .with_rx_ba_win(3);
        let (controller, interfaces) =
            esp_radio::wifi::new(wifi, wifi_config).expect("wifi controller");
        let esp_now_radio = interfaces.esp_now;
        let espnow_status: &'static EmbassyInterfaceStatus = mk_static!(
            EmbassyInterfaceStatus,
            EmbassyInterfaceStatus::new_accounted(
                espnow_core::interface_id(),
                ConnectionState::Initializing,
            )
        );
        let espnow = EspNowInterface::new(
            EspNowAdapter::new(esp_now_radio),
            espnow_channel_policy(),
            ESPNOW_PHY.bitrate,
            espnow_status,
        );
        (controller, espnow, espnow_status)
    };

    let node_identity = node_bootstrap.into_identity();
    let transport_secret = node_identity.transport_secret();
    let destination_secret = node_identity.into_destination_secret();
    let destinations = personal_hopspot_core::HopspotDestinationSet::new(
        destination_secret,
        ANNOUNCE_APP_DATA,
        NODE_ANNOUNCE_APP_DATA,
    );
    #[cfg(feature = "bluetooth-auto")]
    let ble_identity = Some(ble_bootstrap.into_identity());

    let mut manifold_lanes = ManifoldLanes::new();
    let usb_lane = manifold_lanes
        .claim_accounted_interface(
            &USB_MANIFOLD_LANE,
            device_descriptor(USB_INTERFACE_ID),
            &USB_STATUS,
        )
        .expect("USB lane is available");
    #[cfg(feature = "esp-now")]
    let espnow_lane = manifold_lanes
        .claim_accounted_interface(&ESPNOW_MANIFOLD_LANE, espnow.descriptor(), espnow_status)
        .expect("ESP-NOW lane is available");
    #[cfg(feature = "bluetooth-auto")]
    let ble_supervisor_lane = ble_identity.as_ref().map(|_| {
        manifold_lanes
            .claim_supervisor(&BLE_MANIFOLD_LANE, BLE_SUPERVISOR_ID, &BLE_OUTBOUND_WAKE)
            .expect("Bluetooth supervisor lane is available")
    });

    let handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let manifold_wiring = manifold_lanes.into_manifold_wiring(
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );

    let usb_seam = usb_lane.into_seam(NOTIFY.sender(), hardware_entropy);
    spawner.spawn(usb_device_task(usb_rx, usb_tx, usb_seam).expect("usb device task fits"));

    #[cfg(feature = "esp-now")]
    let espnow_seam = espnow_lane.into_seam(NOTIFY.sender(), hardware_entropy);

    #[cfg(feature = "bluetooth-auto")]
    let ble = ble_identity
        .zip(ble_supervisor_lane)
        .map(|(identity, lane)| {
            let fleet: C6BleFleet = lane.into_fleet(NOTIFY.sender(), LIFECYCLE.sender());
            (identity, fleet)
        });
    let host = EmbassyHost::new_with_timebase(timebase, hardware_entropy as fn(&mut [u8]));
    let recipe = PrnsNodeRecipe {
        transport_identity: Some(transport_secret),
        pre_configured_destinations: destinations.into_preconfigured_destinations(),
        app_state: (),
        storage: C6Storage,
        request_endpoints: personal_hopspot_core::node_pages::NodePageRoutes,
        interfaces: personal_rns::runtime::ManuallyAttached,
        persistence: crate::persistence::c6(),
        on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
    };

    static NODE: StaticCell<Node> = StaticCell::new();
    let (node, persistence) =
        PrnsNode::init_static_with_persistence(&NODE, recipe, manifold_wiring, host);
    node.set_protocol_policy(personal_hopspot_core::EMBEDDED_HOPSPOT_PROTOCOL_POLICY);
    static PERSISTENCE: StaticCell<crate::persistence::C6Persistence> = StaticCell::new();
    let persistence = PERSISTENCE.init(persistence);
    spawner.spawn(manifold_task(node, persistence).expect("manifold task fits"));
    #[cfg(feature = "bluetooth-auto")]
    if let Some((identity, fleet)) = ble {
        spawner.spawn(
            ble_task(spawner, bluetooth, mac, identity, fleet, &BLE_SHARED).expect("ble task fits"),
        );
    }
    #[cfg(feature = "esp-now")]
    espnow.run(espnow_seam).await;
    #[cfg(not(feature = "esp-now"))]
    core::future::pending().await
}
