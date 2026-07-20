use super::*;

pub async fn run(spawner: Spawner) {
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

    let mac = base_mac_address();
    let secret_key = c6_secret_key(&mac);

    let transport_secret = secret_key.clone();
    let self_destination = {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key);
        let name = personal_rns::routing::announce::expand_name("lxmf", &["delivery"])
            .expect("valid name");
        personal_rns::routing::announce::derive_destination_hash(&signer.identity_hash(), &name)
    };
    #[cfg(feature = "ble")]
    let mut mac_octets = [0u8; 6];
    #[cfg(feature = "ble")]
    mac_octets.copy_from_slice(&mac.as_bytes()[..6]);

    let seed = self_destination.as_bytes();
    ENTROPY_STATE.store(
        u64::from_le_bytes([
            seed[0], seed[1], seed[2], seed[3], seed[4], seed[5], seed[6], seed[7],
        ]) | 1,
        Ordering::Relaxed,
    );
    let mut inbound: ReactorInbound = HVec::new();
    let mut egress_lanes: ReactorEgressLanes = HVec::new();

    let usb_seam = {
        static IN_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static IN_CH: StaticCell<LaneChannel> = StaticCell::new();
        static OUT_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static OUT_CH: StaticCell<LaneChannel> = StaticCell::new();
        let in_ch = IN_CH.init(zerocopy_channel::Channel::new(IN_BUF.take()));
        let (in_producer, in_consumer) = embassy_grant_lane(in_ch);
        let out_ch = OUT_CH.init(zerocopy_channel::Channel::new(OUT_BUF.take()));
        let (out_producer, out_consumer) = embassy_grant_lane(out_ch);
        let _ = inbound.push((FREE_SLOT, in_consumer));
        let _ = egress_lanes.push((FREE_SLOT, out_producer));
        EmbassyInterfaceSeam::new(
            USB_INTERFACE_ID,
            in_producer,
            NOTIFY.sender(),
            out_consumer,
            seeded_entropy,
        )
    };
    spawner.spawn(usb_device_task(usb_rx, usb_tx, usb_seam).expect("usb device task fits"));

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

    #[cfg(feature = "esp-now")]
    let espnow_seam = {
        static IN_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static IN_CH: StaticCell<LaneChannel> = StaticCell::new();
        static OUT_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static OUT_CH: StaticCell<LaneChannel> = StaticCell::new();
        let in_ch = IN_CH.init(zerocopy_channel::Channel::new(IN_BUF.take()));
        let (in_producer, in_consumer) = embassy_grant_lane(in_ch);
        let out_ch = OUT_CH.init(zerocopy_channel::Channel::new(OUT_BUF.take()));
        let (out_producer, out_consumer) = embassy_grant_lane(out_ch);
        let _ = inbound.push((FREE_SLOT, in_consumer));
        let _ = egress_lanes.push((FREE_SLOT, out_producer));
        EmbassyInterfaceSeam::new(
            espnow.id(),
            in_producer,
            NOTIFY.sender(),
            out_consumer,
            seeded_entropy,
        )
    };

    #[cfg(feature = "ble")]
    let ble_fleet: C6BleFleet = {
        static IN_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static IN_CH: StaticCell<LaneChannel> = StaticCell::new();
        static OUT_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static OUT_CH: StaticCell<LaneChannel> = StaticCell::new();
        let in_ch = IN_CH.init(zerocopy_channel::Channel::new(IN_BUF.take()));
        let (in_producer, in_consumer) = embassy_grant_lane(in_ch);
        let out_ch = OUT_CH.init(zerocopy_channel::Channel::new(OUT_BUF.take()));
        let (mut out_producer, out_consumer) = embassy_grant_lane(out_ch);
        out_producer.set_outbound_wake(&BLE_OUTBOUND_WAKE);
        let _ = inbound.push((FREE_SLOT, in_consumer));
        let _ = egress_lanes.push((FREE_SLOT, out_producer));
        Fleet::new(
            FleetWire {
                inbound: in_producer,
                outbound: out_consumer,
                notify: NOTIFY.sender(),
                outbound_wake: &BLE_OUTBOUND_WAKE,
            },
            LIFECYCLE.sender(),
        )
    };

    let handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let plumbing = ReactorPlumbing::new(
        inbound,
        PooledEgress::new(egress_lanes),
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new_with_timebase(timebase, seeded_entropy as fn(&mut [u8]));

    static NODE: StaticCell<Node> = StaticCell::new();
    let node: &'static mut Node = NODE.init(PrnsNode::new(
        PrnsNodeRecipe {
            transport_identity: Some(transport_secret),
            pre_configured_destinations: [PreConfiguredDestination::Single {
                resource_strategy:
                    personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
                app_name: "lxmf",
                aspects: &["delivery"],
                identity: secret_key,
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
    node.activate(USB_SLOT, device_descriptor(USB_INTERFACE_ID));
    #[cfg(feature = "esp-now")]
    node.activate(ESPNOW_SLOT, espnow.descriptor());
    #[cfg(feature = "ble")]
    node.activate_fleet(BLE_FLEET_SLOT, BLE_FLEET_ID);
    #[cfg(all(feature = "ble", feature = "esp-now"))]
    {
        spawner.spawn(
            ble_task(spawner, p.BT, mac_octets, ble_fleet, &BLE_SHARED).expect("ble task fits"),
        );
        join(
            node.run_reactor_with_interface_store(&INTERFACE_STORE),
            espnow.run(espnow_seam),
        )
        .await;
    }
    #[cfg(all(feature = "esp-now", not(feature = "ble")))]
    {
        join(
            node.run_reactor_with_interface_store(&INTERFACE_STORE),
            espnow.run(espnow_seam),
        )
        .await;
    }
    #[cfg(all(feature = "ble", not(feature = "esp-now")))]
    {
        spawner.spawn(
            ble_task(spawner, p.BT, mac_octets, ble_fleet, &BLE_SHARED).expect("ble task fits"),
        );
        node.run_reactor_with_interface_store(&INTERFACE_STORE)
            .await;
    }
    #[cfg(not(any(feature = "ble", feature = "esp-now")))]
    node.run_reactor_with_interface_store(&INTERFACE_STORE)
        .await;
}
