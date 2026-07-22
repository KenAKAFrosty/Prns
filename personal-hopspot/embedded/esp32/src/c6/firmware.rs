use super::*;

pub async fn run(spawner: Spawner) {
    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(size: HEAP_BYTES);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);
    let (usb_rx, usb_tx) = UsbSerialJtag::new(p.USB_DEVICE).into_async().split();

    let timg0 = TimerGroup::new(p.TIMG0);
    let sw_int = SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let mut rtc = Rtc::new(p.LPWR);
    rtc.rwdt.disable();
    rtc.swd.disable();
    let timebase = EmbassyTimebase::start_at(InstantMillis(rtc.current_time_us() / 1000));

    #[cfg(feature = "esp-now")]
    let (_espnow_controller, espnow, _espnow_status) = {
        let wifi_config = ControllerConfig::default()
            .with_static_rx_buf_num(4)
            .with_rx_ba_win(3);
        let (controller, interfaces) =
            esp_radio::wifi::new(p.WIFI, wifi_config).expect("wifi controller");
        let esp_now_radio = interfaces.esp_now;
        let espnow_status: &'static EmbassyInterfaceStatus = mk_static!(
            EmbassyInterfaceStatus,
            EmbassyInterfaceStatus::new(espnow_core::interface_id(), ConnectionState::Initializing)
        );
        let espnow = EspNowInterface::new(
            EspNowAdapter::new(esp_now_radio),
            espnow_channel_policy(),
            espnow_status,
        );
        (controller, espnow, espnow_status)
    };

    let mac = base_mac_address();
    let node_bootstrap = crate::identity::bootstrap_node_identity();
    crate::identity::log_persistence("node", node_bootstrap.persistence());
    let ble_bootstrap = crate::identity::bootstrap_ble_identity();
    crate::identity::log_persistence("Bluetooth", ble_bootstrap.persistence());
    let node_identity = node_bootstrap.into_identity();
    let transport_secret = node_identity.transport_secret();
    #[cfg(feature = "bluetooth-auto")]
    let mut mac_octets = [0u8; 6];
    #[cfg(feature = "bluetooth-auto")]
    mac_octets.copy_from_slice(&mac.as_bytes()[..6]);
    #[cfg(feature = "bluetooth-auto")]
    let ble_identity = Some(ble_bootstrap.into_identity());

    let mut reactor_pool = REACTOR_POOL.try_take().expect("reactor pool is available");
    let usb_lane = reactor_pool
        .take_interface::<USB_SLOT>()
        .expect("USB lane is available");
    #[cfg(feature = "esp-now")]
    let espnow_lane = reactor_pool
        .take_interface::<ESPNOW_SLOT>()
        .expect("ESP-NOW lane is available");
    #[cfg(feature = "bluetooth-auto")]
    let ble_supervisor_lane = reactor_pool
        .take_supervisor::<BLE_SUPERVISOR_SLOT>(&BLE_OUTBOUND_WAKE)
        .expect("Bluetooth supervisor lane is available");

    let handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let plumbing = reactor_pool.into_plumbing(
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );

    let usb_seam = usb_lane.into_seam(USB_INTERFACE_ID, NOTIFY.sender(), hardware_entropy);
    spawner.spawn(usb_device_task(usb_rx, usb_tx, usb_seam).expect("usb device task fits"));

    #[cfg(feature = "esp-now")]
    let espnow_seam = espnow_lane.into_seam(espnow.id(), NOTIFY.sender(), hardware_entropy);

    #[cfg(feature = "bluetooth-auto")]
    let ble_fleet: Option<C6BleFleet> =
        ble_identity.map(|_| ble_supervisor_lane.into_fleet(NOTIFY.sender(), LIFECYCLE.sender()));
    let host = EmbassyHost::new_with_timebase(timebase, hardware_entropy as fn(&mut [u8]));

    static NODE: StaticCell<Node> = StaticCell::new();
    let node: &'static mut Node = NODE.init(PrnsNode::new(
        PrnsNodeRecipe {
            transport_identity: Some(transport_secret),
            pre_configured_destinations: [PreConfiguredDestination::Single {
                resource_strategy:
                    personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
                app_name: "lxmf",
                aspects: &["delivery"],
                identity: node_identity.into_destination_secret(),
                announce_app_data: ANNOUNCE_APP_DATA,
                proof: personal_rns::routing::ProofStrategy::ProveAll,
                link_requests: personal_rns::routing::LinkRequestPolicy::AcceptAll,
                ratchet: RatchetPolicy::Ratcheted,
                request_handlers: RequestHandlerRegistration::None,
            }],
            app_state: (),
            storage: C6Storage,
            routes: personal_rns::routes![],
            interfaces: personal_rns::runtime::Manual,
            on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
        },
        plumbing,
        host,
        HVec::new(),
    ));
    node.activate(USB_SLOT, device_descriptor(USB_INTERFACE_ID))
        .expect("USB activation fits the declared topology");
    #[cfg(feature = "esp-now")]
    node.activate(ESPNOW_SLOT, espnow.descriptor())
        .expect("ESP-NOW activation fits the declared topology");
    #[cfg(feature = "bluetooth-auto")]
    if ble_identity.is_some() {
        node.activate_supervisor(BLE_SUPERVISOR_SLOT, BLE_SUPERVISOR_ID)
            .expect("Bluetooth supervisor activation fits the declared topology");
    }
    #[cfg(all(feature = "bluetooth-auto", feature = "esp-now"))]
    {
        if let (Some(identity), Some(fleet)) = (ble_identity, ble_fleet) {
            spawner.spawn(
                ble_task(spawner, p.BT, mac_octets, identity, fleet, &BLE_SHARED)
                    .expect("ble task fits"),
            );
        }
        join(
            node.run_reactor_with_interface_store(&INTERFACE_STORE),
            espnow.run(espnow_seam),
        )
        .await;
    }
    #[cfg(all(feature = "esp-now", not(feature = "bluetooth-auto")))]
    {
        join(
            node.run_reactor_with_interface_store(&INTERFACE_STORE),
            espnow.run(espnow_seam),
        )
        .await;
    }
    #[cfg(all(feature = "bluetooth-auto", not(feature = "esp-now")))]
    {
        if let (Some(identity), Some(fleet)) = (ble_identity, ble_fleet) {
            spawner.spawn(
                ble_task(spawner, p.BT, mac_octets, identity, fleet, &BLE_SHARED)
                    .expect("ble task fits"),
            );
        }
        node.run_reactor_with_interface_store(&INTERFACE_STORE)
            .await;
    }
    #[cfg(not(any(feature = "bluetooth-auto", feature = "esp-now")))]
    node.run_reactor_with_interface_store(&INTERFACE_STORE)
        .await;
}
