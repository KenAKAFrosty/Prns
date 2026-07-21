use super::*;

pub(crate) async fn run<B: Esp32S3Board>(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);
    let bringup = B::bringup(p, &spawner).await;
    run_core::<B>(spawner, bringup).await;
}

/// Platform run on core 0: the self-identity crypto, the radios + WiFi/TCP, and the I/O
/// run-loops + screen. The engine is built *and* owned by core 1 (the construction transient,
/// then the reactor, on its own stack), so core 0 never touches the node. Never returns.
#[allow(clippy::too_many_lines)]
pub(super) async fn run_core<B: Esp32S3Board>(
    spawner: Spawner,
    b: Bringup<B::Display, B::Battery>,
) {
    log_heap_footprint("run_core entry (post-bringup, core 0)");
    let mut display = b.display;
    let oled_ok = b.oled_ok;
    let mut battery_source = b.battery;
    #[cfg(feature = "wifi-auto")]
    let wifi_config = hopspot_wifi_config();
    #[cfg(feature = "wifi-auto")]
    let station_configured = wifi_config.has_station();
    #[cfg(not(feature = "wifi-auto"))]
    let station_configured = false;
    let radio_mode = boot_radio_mode(station_configured);

    let usb_status = B::usb_status();
    let usb_id = usb_status.id();
    let (usb_rx, usb_tx) = UsbSerialJtag::new(b.usb_device).into_async().split();

    let mac = base_mac_address();
    let mut mac_octets = [0u8; 6];
    mac_octets.copy_from_slice(&mac.as_bytes()[..6]);
    let secret_key = fixture_identity_secret_key(&mac);

    let transport_secret = secret_key.clone();
    let self_destination = {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key);
        let name = personal_rns::routing::announce::expand_name("lxmf", &["delivery"])
            .expect("valid name");
        personal_rns::routing::announce::derive_destination_hash(&signer.identity_hash(), &name)
    };
    let seed = self_destination.as_bytes();
    ENTROPY_STATE.store(
        u64::from_le_bytes([
            seed[0], seed[1], seed[2], seed[3], seed[4], seed[5], seed[6], seed[7],
        ]) | 1,
        Ordering::Relaxed,
    );

    let mut inbound: ReactorInbound = HVec::new();
    let mut egress_lanes: ReactorEgressLanes = HVec::new();
    let mut iface_halves: [Option<(
        EmbassyGrantProducer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
        EmbassyGrantConsumer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
    )>; IFACES] = [const { None }; IFACES];
    for slot in 0..IFACES {
        let in_ch = IN_CH[slot].init(zerocopy_channel::Channel::new(IN_BUF[slot].take()));
        let (in_producer, in_consumer) = embassy_grant_lane(in_ch);
        let out_ch = OUT_CH[slot].init(zerocopy_channel::Channel::new(OUT_BUF[slot].take()));
        let (mut out_producer, out_consumer) = embassy_grant_lane(out_ch);
        if slot == WIFI_FLEET_SLOT {
            out_producer.set_outbound_wake(&OUTBOUND_WAKE);
        }
        #[cfg(feature = "bluetooth-auto")]
        if slot == BLE_FLEET_SLOT {
            out_producer.set_outbound_wake(&BLE_OUTBOUND_WAKE);
        }
        let _ = inbound.push((FREE_SLOT, in_consumer));
        let _ = egress_lanes.push((FREE_SLOT, out_producer));
        iface_halves[slot] = Some((in_producer, out_consumer));
    }

    #[cfg(feature = "lora")]
    let lora_radio = b.lora_radio;
    let lora_profile = DEFAULT_915_PROFILE;
    let lora_id = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&lora_profile));
    let lora_status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new(lora_id, ConnectionState::Initializing)
    );
    #[cfg(feature = "lora")]
    let lora = LoRaInterface::new(
        lora_radio,
        lora_profile,
        &LORA_CONTROL,
        lora_status,
        LIFECYCLE.dyn_sender(),
    );

    // The WiFi stack carries both the WiFi-auto UDP and the TCP client, so it stands up before the
    // node moves to core 1 — activating the TCP slot is a core-0-only act.
    #[cfg(feature = "wifi-auto")]
    let (wifi, tcp_stack, esp_now) = build_wifi(
        &spawner,
        b.wifi,
        mac_octets,
        &wifi_config,
        radio_mode == RadioMode::AccessPoint,
    );
    #[cfg(not(feature = "wifi-auto"))]
    let wifi: Option<AutoWifi<'static, MEMBERS>> = None;
    #[cfg(not(feature = "wifi-auto"))]
    let tcp_stack: Option<Stack<'static>> = None;

    #[cfg(feature = "esp-now")]
    let espnow_status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new(espnow_core::interface_id(), ConnectionState::Initializing)
    );
    #[cfg(feature = "esp-now")]
    let espnow = esp_now.map(|radio| {
        EspNowInterface::new(
            EspNowAdapter::new(radio),
            espnow_channel_policy(station_configured),
            espnow_status,
        )
    });

    let tcp_built = tcp_stack.and_then(build_tcp);
    let tcp_status = tcp_built.as_ref().map(|(_, status, _)| *status);
    let tcp_id = tcp_built.as_ref().map(|(_, _, id)| *id);

    let handle: Handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let plumbing = ReactorPlumbing::new(
        inbound,
        PooledEgress::new(egress_lanes),
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new_with_timebase(b.timebase, seeded_entropy as fn(&mut [u8]));

    let recipe = PrnsNodeRecipe {
        transport_identity: Some(transport_secret),
        pre_configured_destinations: [PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: "lxmf",
            aspects: &["delivery"],
            identity: secret_key,
            announce_app_data: B::ANNOUNCE_APP_DATA,
            proof: personal_rns::routing::ProofStrategy::ProveAll,
            link_requests: personal_rns::routing::LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::Ratcheted,
            request_handlers: RequestHandlerRegistration::None,
        }],
        app_state: (),
        storage: EngineStorageType::default(),
        routes: personal_rns::routes![],
        interfaces: personal_rns::runtime::Manual,
        on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
    };

    #[cfg(feature = "lora")]
    let lora_cfg = lora.descriptor();
    #[cfg(feature = "esp-now")]
    let espnow_cfg = espnow.as_ref().map(|e| e.descriptor());
    let tcp_cfg = tcp_built.as_ref().map(|(t, _, _)| t.descriptor());
    let has_wifi = wifi.is_some();

    // The engine is built and run on core 1: its stack carries the dalek-heavy construction
    // transient, then the reactor reuses that space (see `CORE1_STACK_BYTES`).
    let core1_stack = mk_static!(CpuStack<CORE1_STACK_BYTES>, CpuStack::new());
    esp_rtos::start_second_core(b.cpu_ctrl, b.sw_int1, core1_stack, move || {
        static NODE: StaticCell<S3Node> = StaticCell::new();
        let node: &'static mut S3Node =
            NODE.init_with(|| PrnsNode::new(recipe, plumbing, host, HVec::new()));
        node.activate(USB_SLOT, device_descriptor(usb_id));
        if let Some(cfg) = tcp_cfg {
            node.activate(TCP_SLOT, cfg);
        }
        #[cfg(feature = "lora")]
        node.activate(LORA_SLOT, lora_cfg);
        #[cfg(feature = "esp-now")]
        if let Some(cfg) = espnow_cfg {
            node.activate(ESPNOW_SLOT, cfg);
        }
        #[cfg(feature = "wifi-auto")]
        if has_wifi {
            node.activate_fleet(WIFI_FLEET_SLOT, WIFI_FLEET_ID);
        }
        #[cfg(feature = "bluetooth-auto")]
        if radio_mode == RadioMode::Ble {
            node.activate_fleet(BLE_FLEET_SLOT, BLE_FLEET_ID);
        }
        log_heap_footprint("post-construction (engine columns boxed into PSRAM)");

        static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
        EXECUTOR
            .init(esp_rtos::embassy::Executor::new())
            .run(|spawner| {
                spawner.spawn(reactor_core(node).expect("reactor task fits"));
            })
    });

    let usb_seam = {
        let (in_producer, out_consumer) = iface_halves[USB_SLOT].take().expect("usb slot half");
        EmbassyInterfaceSeam::new(
            usb_id,
            in_producer,
            NOTIFY.sender(),
            out_consumer,
            seeded_entropy,
        )
    };
    spawner.spawn(
        usb_device_task(usb_rx, usb_tx, usb_seam, usb_id, usb_status).expect("usb task fits"),
    );

    #[cfg(feature = "lora")]
    let lora_seam = {
        let (lora_in_producer, lora_out_consumer) =
            iface_halves[LORA_SLOT].take().expect("lora slot half");
        EmbassyInterfaceSeam::new(
            lora_id,
            lora_in_producer,
            NOTIFY.sender(),
            lora_out_consumer,
            seeded_entropy,
        )
    };

    #[cfg(feature = "esp-now")]
    let espnow = espnow.map(|interface| {
        let (in_producer, out_consumer) =
            iface_halves[ESPNOW_SLOT].take().expect("espnow slot half");
        let seam = EmbassyInterfaceSeam::new(
            interface.id(),
            in_producer,
            NOTIFY.sender(),
            out_consumer,
            seeded_entropy,
        );
        (interface, seam)
    });

    let tcp = tcp_built.map(|(tcp, _, _)| {
        let (in_producer, out_consumer) = iface_halves[TCP_SLOT].take().expect("tcp slot half");
        let seam = EmbassyInterfaceSeam::new(
            tcp.id(),
            in_producer,
            NOTIFY.sender(),
            out_consumer,
            seeded_entropy,
        );
        (tcp, seam)
    });

    #[cfg(feature = "wifi-auto")]
    let wifi_fleet: Fleet<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP> = {
        let (in_producer, out_consumer) = iface_halves[WIFI_FLEET_SLOT]
            .take()
            .expect("wifi fleet half");
        Fleet::new(
            FleetWire {
                inbound: in_producer,
                outbound: out_consumer,
                notify: NOTIFY.sender(),
                outbound_wake: &OUTBOUND_WAKE,
            },
            LIFECYCLE.sender(),
        )
    };
    // The WiFi-auto run loop's two MTU receive buffers live on the heap (the D-cache donation),
    // not on the bounded `#[esp_rtos::main]` stack that run()'s future rides; the alloc-free
    // embassy AutoWifi just borrows them. Leaked: they live for the program's whole life anyway.
    #[cfg(feature = "wifi-auto")]
    let wifi_data_buf: &'static mut [u8] =
        alloc::vec![0u8; wifi_auto_contract::HARDWARE_MTU].leak();
    #[cfg(feature = "wifi-auto")]
    let wifi_sec_data_buf: &'static mut [u8] =
        alloc::vec![0u8; wifi_auto_contract::HARDWARE_MTU].leak();
    #[cfg(feature = "bluetooth-auto")]
    let ble_fleet: Fleet<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP> = {
        let (in_producer, out_consumer) =
            iface_halves[BLE_FLEET_SLOT].take().expect("ble fleet half");
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

    let button = Input::new(b.button, InputConfig::default().with_pull(Pull::Up));
    spawner.spawn(button_task(button).expect("button task fits"));

    let wifi_status = wifi.as_ref().map(AutoWifi::status);
    let wifi_id = wifi_status.as_ref().map(|status| {
        use personal_rns::interfaces::InterfaceStatus;
        status.id()
    });

    #[cfg(feature = "esp-now")]
    let espnow_card_id = espnow.as_ref().map(|(interface, _)| interface.id());
    #[cfg(feature = "esp-now")]
    let espnow_card_status = espnow_card_id.map(|_| espnow_status);
    #[cfg(not(feature = "esp-now"))]
    let (espnow_card_id, espnow_card_status): (
        Option<InterfaceId>,
        Option<&'static EmbassyInterfaceStatus>,
    ) = (None, None);

    let render = async move {
        let mut ui_state = screen::UiState::new();
        ui_state.set_storage_limits(<EngineStorageType as StorageLayout>::LIMITS);
        ui_state.set_display_power_capable(oled_ok);
        ui_state.set_radio_state(
            cfg!(feature = "wifi-auto"),
            radio_mode == RadioMode::AccessPoint,
        );
        let mut working_lora_profile = DEFAULT_915_PROFILE;
        let mut battery_state = screen::BatteryState::Unknown;
        let mut battery_gauge = screen::BatteryGauge::lipo();
        #[cfg(feature = "wifi-auto")]
        let ap_footer_ssid = (radio_mode == RadioMode::AccessPoint).then(ap_ssid);
        #[cfg(feature = "wifi-auto")]
        let site_footer = ap_footer_ssid.as_deref().map(|ssid| {
            screen::UiFooter::with_lines(
                "WifiAP",
                Some(ssid),
                Some("docs @"),
                Some(CAPTIVE_PORTAL_HOST),
            )
        });
        #[cfg(not(feature = "wifi-auto"))]
        let site_footer = None;
        let has_site_footer = site_footer.is_some();
        let mut ticks_to_battery: u8 = 0;
        let mut activity = screen::CardActivityTracker::<8>::new();
        let mut notice_until_ms: Option<u64> = None;
        let mut oled_awake = true;
        let mut oled_off_at_ms: Option<u64> = None;
        let mut oled_sleep_at_ms: Option<u64> = None;
        let mut render_tick = Ticker::every(RENDER_INTERVAL);
        let mut settle_after_draw = false;
        loop {
            if ticks_to_battery == 0 {
                battery_state = battery_gauge.sample(&mut battery_source);
                ticks_to_battery = RENDER_TICKS_PER_BATTERY;
            }

            let snapshots = build_snapshots(
                usb_status,
                wifi_status.as_ref(),
                tcp_status,
                lora_status,
                espnow_card_status,
            );
            let mut cards = build_cards(
                &snapshots,
                usb_status.id(),
                wifi_id,
                tcp_id,
                lora_status.id(),
                espnow_card_id,
            );
            let now_ms = embassy_time::Instant::now().as_millis();
            let activity_secs = (now_ms / 1000).min(u64::from(u32::MAX)) as u32;
            activity.update(&mut cards, activity_secs);
            let card_count = cards.len();
            #[cfg(feature = "wifi-auto")]
            let menu_ap_ssid = ap_footer_ssid.as_deref();
            #[cfg(feature = "wifi-auto")]
            let interface_menu_details = build_interface_menu_details(
                ui_state
                    .selected_card(card_count)
                    .and_then(|index| cards.get(index)),
                &snapshots,
                usb_status,
                &wifi_config,
                menu_ap_ssid,
            );
            #[cfg(not(feature = "wifi-auto"))]
            let interface_menu_details = screen::InterfaceMenuDetailRows::new();
            ui_state.sync_card_count_with_footer(card_count, has_site_footer);
            if notice_until_ms.is_some_and(|until| now_ms >= until) {
                ui_state.clear_notice();
                notice_until_ms = None;
            }
            if let Some(off_at) = oled_off_at_ms {
                if oled_awake && now_ms >= off_at {
                    B::set_display_awake(&mut display, false);
                    oled_awake = false;
                    oled_off_at_ms = None;
                    ui_state.clear_notice();
                    notice_until_ms = None;
                }
            }
            if let Some(sleep_at) = oled_sleep_at_ms {
                if oled_awake && now_ms >= sleep_at {
                    B::set_display_awake(&mut display, false);
                    oled_awake = false;
                }
            }
            if oled_ok && oled_awake {
                screen::draw_with_state_footer_details_at(
                    &mut display,
                    &cards,
                    battery_state,
                    &ui_state,
                    site_footer,
                    &interface_menu_details,
                    now_ms,
                );
                B::flush(&mut display);
            }
            if settle_after_draw {
                Timer::after(Duration::from_millis(screen::COALESCE_MS)).await;
                settle_after_draw = false;
            }

            match select3(
                INTERFACE_STORE.changed(),
                BUTTON_EVENTS.receive(),
                render_tick.next(),
            )
            .await
            {
                Either3::First(()) => {
                    settle_after_draw = true;
                }
                Either3::Third(()) => {
                    ticks_to_battery = ticks_to_battery.saturating_sub(1);
                }
                Either3::Second(event) => {
                    let now_ms = embassy_time::Instant::now().as_millis();
                    if !oled_awake && oled_sleep_at_ms.is_none() {
                        if oled_ok {
                            B::set_display_awake(&mut display, true);
                            oled_awake = true;
                        }
                        oled_off_at_ms = None;
                        ui_state.show_notice(screen::UiNotice::Awake);
                        notice_until_ms = Some(now_ms + NOTICE_MS);
                        continue;
                    }
                    oled_off_at_ms = None;
                    let selected_kind = ui_state
                        .selected_card(card_count)
                        .and_then(|index| cards.get(index))
                        .map(|card| card.kind);
                    match ui_state.handle_input_with_footer(
                        event,
                        card_count,
                        has_site_footer,
                        selected_kind,
                    ) {
                        screen::UiAction::OledOff => {
                            ui_state.show_notice(screen::UiNotice::OledOff);
                            notice_until_ms = Some(now_ms + NOTICE_MS);
                            oled_off_at_ms = Some(now_ms + NOTICE_MS);
                        }
                        screen::UiAction::Sleep => {
                            ui_state.show_notice(screen::UiNotice::Sleeping);
                            notice_until_ms = Some(now_ms + NOTICE_MS);
                            oled_sleep_at_ms = Some(now_ms + OLED_SLEEP_DELAY_MS);
                            usb_status.disable();
                            lora_status.disable();
                            if let Some(status) = wifi_status.as_ref() {
                                status.disable();
                            }
                            if let Some(status) = espnow_card_status {
                                status.disable();
                            }
                            if let Some(tcp) = tcp_status {
                                tcp.disable();
                            }
                            #[cfg(feature = "bluetooth-auto")]
                            {
                                let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                status.disable();
                            }
                        }
                        screen::UiAction::Wake => {
                            oled_off_at_ms = None;
                            oled_sleep_at_ms = None;
                            if oled_ok && !oled_awake {
                                B::set_display_awake(&mut display, true);
                                oled_awake = true;
                            }
                            ui_state.show_notice(screen::UiNotice::Awake);
                            notice_until_ms = Some(now_ms + NOTICE_MS);
                            usb_status.enable();
                            lora_status.enable();
                            if let Some(status) = wifi_status.as_ref() {
                                status.enable();
                            }
                            if let Some(status) = espnow_card_status {
                                status.enable();
                            }
                            if let Some(tcp) = tcp_status {
                                tcp.enable();
                            }
                            #[cfg(feature = "bluetooth-auto")]
                            {
                                let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                status.enable();
                            }
                        }
                        screen::UiAction::Announce => {
                            ui_state.show_notice(screen::UiNotice::Announcing);
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                            let _ = handle.issue(EngineCommand::AnnounceNow(AnnounceNow {
                                destination: self_destination,
                                target: AnnounceTarget::AllInterfaces,
                                app_data: AnnounceAppData::Registered,
                            }));
                        }
                        screen::UiAction::ToggleSelectedInterface => {
                            if let Some(card) = ui_state
                                .selected_card(card_count)
                                .and_then(|index| cards.get(index))
                            {
                                let mut handled = false;
                                let mut show_toggle_notice = |enabled: bool| {
                                    ui_state.show_notice(if enabled {
                                        screen::UiNotice::TurningOff
                                    } else {
                                        screen::UiNotice::TurningOn
                                    });
                                    notice_until_ms =
                                        Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                                };
                                if card.id == usb_status.id() {
                                    show_toggle_notice(usb_status.is_enabled());
                                    usb_status.toggle_enabled();
                                    handled = true;
                                }
                                if !handled && card.id == lora_status.id() {
                                    show_toggle_notice(lora_status.is_enabled());
                                    lora_status.toggle_enabled();
                                    handled = true;
                                }
                                if !handled {
                                    if let Some(status) = wifi_status.as_ref() {
                                        if card.id == status.id() {
                                            show_toggle_notice(status.is_enabled());
                                            status.toggle_enabled();
                                            handled = true;
                                        }
                                    }
                                }
                                if !handled && Some(card.id) == espnow_card_id {
                                    if let Some(status) = espnow_card_status {
                                        show_toggle_notice(status.is_enabled());
                                        status.toggle_enabled();
                                        handled = true;
                                    }
                                }
                                if !handled {
                                    if let (Some(tcp), Some(tcp_id)) = (tcp_status, tcp_id) {
                                        if card.id == tcp_id {
                                            show_toggle_notice(tcp.is_enabled());
                                            tcp.toggle_enabled();
                                            #[cfg(feature = "bluetooth-auto")]
                                            {
                                                handled = true;
                                            }
                                        }
                                    }
                                }
                                #[cfg(feature = "bluetooth-auto")]
                                if !handled && card.id == BLE_FLEET_ID {
                                    let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                    show_toggle_notice(status.is_enabled());
                                    status.toggle_enabled();
                                }
                            }
                        }
                        screen::UiAction::OpenLoRaEditor => {
                            ui_state.open_lora_editor(working_lora_profile);
                        }
                        screen::UiAction::SetLoRaProfile(profile) => {
                            ui_state.show_notice(screen::UiNotice::Saved);
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                            working_lora_profile = profile;
                            LORA_CONTROL.signal(profile);
                        }
                        screen::UiAction::SwapRadioMode => {
                            #[cfg(feature = "wifi-auto")]
                            {
                                let next = match radio_mode {
                                    RadioMode::Ble => RadioMode::AccessPoint,
                                    RadioMode::AccessPoint => RadioMode::Ble,
                                };
                                request_radio_mode(next);
                            }
                        }
                        screen::UiAction::OpenDocs => {}
                        screen::UiAction::None => {}
                    }
                }
            }
        }
    };

    #[cfg(all(feature = "bluetooth-auto", not(feature = "wifi-auto")))]
    // Halve the BLE controller task stack (8192 -> 4096; esp-radio's own default hints "4096?") to
    // reclaim ~4 KiB internal SRAM toward the full radio stack + SoftAP fit.
    let ble_connector = esp_radio::ble::controller::BleConnector::new(
        b.bt,
        esp_radio::ble::Config::default().with_task_stack_size(4096),
    )
    .expect("ble connector");

    #[cfg(all(feature = "bluetooth-auto", not(feature = "wifi-auto")))]
    {
        let _ = (wifi, tcp, has_wifi);
        join(
            crate::bluetooth_auto::run(ble_connector, mac_octets, ble_fleet, &BLE_SHARED),
            render,
        )
        .await;
    }
    #[cfg(all(feature = "wifi-auto", not(feature = "bluetooth-auto")))]
    {
        let lora_run = lora.run(lora_seam);
        let espnow_run = async {
            if let Some((interface, seam)) = espnow {
                interface.run(seam).await;
            }
        };
        match (wifi, tcp) {
            (Some(wifi), Some((tcp, tcp_seam))) => {
                join(
                    join(
                        join(lora_run, espnow_run),
                        join(
                            wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                            tcp.run(tcp_seam),
                        ),
                    ),
                    render,
                )
                .await;
            }
            (Some(wifi), None) => {
                join(
                    join(
                        join(lora_run, espnow_run),
                        wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                    ),
                    render,
                )
                .await;
            }
            (None, _) => {
                join(join(lora_run, espnow_run), render).await;
            }
        }
    }
    #[cfg(all(feature = "bluetooth-auto", feature = "wifi-auto"))]
    {
        let lora_run = lora.run(lora_seam);
        let espnow_run = async {
            if let Some((interface, seam)) = espnow {
                interface.run(seam).await;
            }
        };
        match radio_mode {
            RadioMode::Ble => {
                log_heap_footprint("pre-ble-connector (core 0)");
                let ble_connector = esp_radio::ble::controller::BleConnector::new(
                    b.bt,
                    esp_radio::ble::Config::default().with_task_stack_size(4096),
                )
                .expect("ble connector");
                log_heap_footprint("post-ble-connector (core 0)");
                let ble_run =
                    crate::bluetooth_auto::run(ble_connector, mac_octets, ble_fleet, &BLE_SHARED);
                match (wifi, tcp) {
                    (Some(wifi), Some((tcp, tcp_seam))) => {
                        join(
                            join(join(join(ble_run, lora_run), espnow_run), tcp.run(tcp_seam)),
                            join(
                                wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                                render,
                            ),
                        )
                        .await;
                    }
                    (Some(wifi), None) => {
                        join(
                            join(join(ble_run, lora_run), espnow_run),
                            join(
                                wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                                render,
                            ),
                        )
                        .await;
                    }
                    (None, _) => {
                        join(join(join(ble_run, lora_run), espnow_run), render).await;
                    }
                }
            }
            RadioMode::AccessPoint => {
                let _ = (b.bt, ble_fleet);
                match (wifi, tcp) {
                    (Some(wifi), Some((tcp, tcp_seam))) => {
                        join(
                            join(
                                join(lora_run, espnow_run),
                                join(
                                    wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                                    tcp.run(tcp_seam),
                                ),
                            ),
                            render,
                        )
                        .await;
                    }
                    (Some(wifi), None) => {
                        join(
                            join(
                                join(lora_run, espnow_run),
                                wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                            ),
                            render,
                        )
                        .await;
                    }
                    (None, _) => {
                        join(join(lora_run, espnow_run), render).await;
                    }
                }
            }
        }
    }
}

/// Core 1: run only the engine reactor over the slot pool. The node was built on core 0 and lives in
/// a `static`; core 1 borrows it by `&'static mut`, so only a pointer crosses the core boundary (the
/// engine never moves) and this core needs just a small per-poll stack for the ingest crypto.
#[embassy_executor::task]
async fn reactor_core(node: &'static mut S3Node) {
    node.run_reactor_with_interface_store(&INTERFACE_STORE)
        .await
}

/// A bring-up fixture identity (the oracle X25519 0x22 ‖ Ed25519 0x11 keypair with the board MAC
/// mixed in so every flashed board is distinct). NEVER ship: predictable from the MAC.
fn fixture_identity_secret_key(
    mac: &esp_hal::efuse::MacAddress,
) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret_key[..32].fill(0x22);
    secret_key[32..].fill(0x11);
    for (i, byte) in mac.as_bytes().iter().enumerate() {
        secret_key[i] ^= byte;
        secret_key[32 + i] ^= byte;
    }
    secret_key
}
