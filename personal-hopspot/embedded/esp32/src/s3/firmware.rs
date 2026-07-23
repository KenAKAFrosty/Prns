use super::*;

pub(crate) async fn run<B: Esp32S3Board>(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);
    let bringup = B::bringup(p).await;
    run_core::<B>(spawner, bringup).await;
}

#[allow(clippy::too_many_lines)]
pub(super) async fn run_core<B: Esp32S3Board>(
    spawner: Spawner,
    hardware: S3BoardHardware<B::Display, B::Battery>,
) {
    let BoardFace {
        display,
        battery,
        button,
    } = hardware.face;
    let BoardDisplay {
        device: mut display,
        initialized: oled_ok,
    } = display;
    let mut battery_source = battery;
    let S3InterfaceHardware {
        usb_device,
        #[cfg(feature = "lora")]
        lora_radio,
        #[cfg(feature = "wifi-auto")]
            wifi: wifi_hardware,
        #[cfg(feature = "bluetooth-auto")]
        bluetooth,
    } = hardware.interface_hardware;
    let S3ReactorHardware {
        cpu_control,
        software_interrupt,
        timebase,
        _rtc,
    } = hardware.reactor;
    #[cfg(feature = "wifi-auto")]
    let wifi_config = hopspot_wifi_config();
    #[cfg(feature = "wifi-auto")]
    let station_configured = wifi_config.has_station();
    #[cfg(not(feature = "wifi-auto"))]
    let station_configured = false;
    let radio_mode = boot_radio_mode(station_configured);

    let usb_status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new(B::USB_INTERFACE_ID, ConnectionState::Initializing)
    );
    let usb_id = usb_status.id();
    let (usb_rx, usb_tx) = UsbSerialJtag::new(usb_device).into_async().split();

    let mac = base_mac_address();
    let mut mac_octets = [0u8; 6];
    mac_octets.copy_from_slice(&mac.as_bytes()[..6]);

    let mut reactor_lanes = ReactorLanes::new();

    #[cfg(feature = "lora")]
    let lora_profile = DEFAULT_915_PROFILE;
    let lora_id = LoRaInterface::<
        ExclusiveDevice<Spi<'static, esp_hal::Async>, Output<'static>, Delay>,
        Input<'static>,
        Input<'static>,
        Output<'static>,
        Delay,
    >::interface_id(&lora_profile);
    let lora_status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new(lora_id, ConnectionState::Initializing)
    );
    #[cfg(feature = "lora")]
    let lora = LoRaInterface::new(LoRaInterfaceInput {
        radio: lora_radio,
        profile: lora_profile,
        control: &LORA_CONTROL,
        status: lora_status,
        lifecycle: LIFECYCLE.dyn_sender(),
    });

    // The WiFi stack carries both the WiFi-auto UDP and the TCP client, so it stands up before the
    // node moves to core 1 — activating the TCP slot is a core-0-only act.
    #[cfg(feature = "wifi-auto")]
    let (wifi, tcp_stack, esp_now) = build_wifi(
        &spawner,
        wifi_hardware,
        mac_octets,
        &wifi_config,
        radio_mode == RadioMode::AccessPoint,
    );
    #[cfg(not(feature = "wifi-auto"))]
    let wifi: Option<AutoWifi<'static, MEMBERS>> = None;
    #[cfg(not(feature = "wifi-auto"))]
    let tcp_stack: Option<Stack<'static>> = None;
    let node_bootstrap = crate::identity::bootstrap_node_identity();
    crate::identity::log_persistence("node", node_bootstrap.persistence());
    let ble_bootstrap = crate::identity::bootstrap_ble_identity();
    crate::identity::log_persistence("Bluetooth", ble_bootstrap.persistence());
    let identity_startup_notice =
        crate::identity::startup_notice(node_bootstrap.persistence(), ble_bootstrap.persistence());
    let node_identity = node_bootstrap.into_identity();
    let transport_secret = node_identity.transport_secret();
    let self_destination = {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(node_identity.secret());
        let name = personal_rns::routing::announce::expand_name("lxmf", &["delivery"])
            .expect("valid name");
        personal_rns::routing::announce::derive_destination_hash(&signer.identity_hash(), &name)
    };
    let ble_identity = Some(ble_bootstrap.into_identity());

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

    let recipe = PrnsNodeRecipe {
        transport_identity: Some(transport_secret),
        pre_configured_destinations: [PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: "lxmf",
            aspects: &["delivery"],
            identity: node_identity.into_destination_secret(),
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

    let usb_lane = reactor_lanes
        .claim_interface(&USB_REACTOR_LANE, device_descriptor(usb_id))
        .expect("USB lane is available");
    let tcp_lane = tcp_cfg.map(|descriptor| {
        reactor_lanes
            .claim_interface(&TCP_REACTOR_LANE, descriptor)
            .expect("TCP lane is available")
    });
    #[cfg(feature = "wifi-auto")]
    let wifi_supervisor_lane = has_wifi.then(|| {
        reactor_lanes
            .claim_supervisor(&WIFI_REACTOR_LANE, WIFI_SUPERVISOR_ID, &OUTBOUND_WAKE)
            .expect("WiFi supervisor lane is available")
    });
    #[cfg(feature = "lora")]
    let lora_lane = reactor_lanes
        .claim_interface(&LORA_REACTOR_LANE, lora_cfg)
        .expect("LoRa lane is available");
    #[cfg(feature = "bluetooth-auto")]
    let ble_supervisor_lane = (radio_mode == RadioMode::Ble && ble_identity.is_some()).then(|| {
        reactor_lanes
            .claim_supervisor(&BLE_REACTOR_LANE, BLE_SUPERVISOR_ID, &BLE_OUTBOUND_WAKE)
            .expect("Bluetooth supervisor lane is available")
    });
    #[cfg(feature = "esp-now")]
    let espnow_lane = espnow_cfg.map(|descriptor| {
        reactor_lanes
            .claim_interface(&ESPNOW_REACTOR_LANE, descriptor)
            .expect("ESP-NOW lane is available")
    });

    let handle: Handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let reactor_wiring = reactor_lanes.into_reactor_wiring(
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new_with_timebase(timebase, hardware_entropy as fn(&mut [u8]));

    let core1_stack = mk_static!(CpuStack<CORE1_STACK_BYTES>, CpuStack::new());
    esp_rtos::start_second_core(cpu_control, software_interrupt, core1_stack, move || {
        static NODE: StaticCell<S3Node> = StaticCell::new();
        let node: &'static mut S3Node = PrnsNode::init_static(&NODE, recipe, reactor_wiring, host);

        static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
        EXECUTOR
            .init(esp_rtos::embassy::Executor::new())
            .run(|spawner| {
                spawner.spawn(reactor_task(node).expect("reactor task fits"));
            })
    });

    let usb_seam = usb_lane.into_seam(NOTIFY.sender(), hardware_entropy);
    spawner.spawn(usb_device_task(usb_rx, usb_tx, usb_seam, usb_status).expect("usb task fits"));

    #[cfg(feature = "lora")]
    let lora_seam = lora_lane.into_seam(NOTIFY.sender(), hardware_entropy);

    #[cfg(feature = "esp-now")]
    let espnow = espnow.zip(espnow_lane).map(|(interface, lane)| {
        let seam = lane.into_seam(NOTIFY.sender(), hardware_entropy);
        (interface, seam)
    });

    let tcp = tcp_built.zip(tcp_lane).map(|((tcp, _, _), lane)| {
        let seam = lane.into_seam(NOTIFY.sender(), hardware_entropy);
        (tcp, seam)
    });

    #[cfg(feature = "wifi-auto")]
    let wifi = wifi.zip(wifi_supervisor_lane).map(|(interface, lane)| {
        let fleet: Fleet<Mtx, { wifi_auto_contract::HARDWARE_MTU }, NOTIFY_CAP, LIFECYCLE_CAP> =
            lane.into_fleet(NOTIFY.sender(), LIFECYCLE.sender());
        (interface, fleet)
    });
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
    let ble = ble_identity
        .zip(ble_supervisor_lane)
        .map(|(identity, lane)| {
            let fleet: S3BleFleet = lane.into_fleet(NOTIFY.sender(), LIFECYCLE.sender());
            (identity, fleet)
        });

    spawner.spawn(button_task(button).expect("button task fits"));

    let wifi_status = wifi.as_ref().map(|(interface, _)| interface.status());
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
        let access_point = if !cfg!(feature = "wifi-auto") {
            screen::AccessPointState::Unsupported
        } else if radio_mode == RadioMode::AccessPoint {
            screen::AccessPointState::Active
        } else {
            screen::AccessPointState::Inactive
        };
        let mut ui_state = screen::UiState::new(screen::UiConfiguration {
            storage_limits: <EngineStorageType as StorageLayout>::LIMITS,
            display_power_control: if oled_ok {
                screen::DisplayPowerControl::Available
            } else {
                screen::DisplayPowerControl::Unavailable
            },
            access_point,
        });
        if let Some(notice) = identity_startup_notice {
            ui_state.show_notice(notice);
        }
        let mut working_lora_profile = DEFAULT_915_PROFILE;
        let mut battery_state = screen::BatteryState::Unknown;
        let mut battery_gauge = screen::BatteryGauge::lipo();
        #[cfg(feature = "wifi-auto")]
        let active_ap_ssid = (radio_mode == RadioMode::AccessPoint).then(ap_ssid);
        #[cfg(feature = "wifi-auto")]
        let local_docs = active_ap_ssid
            .as_deref()
            .map(|wifi_ssid| screen::LocalDocsAccess {
                wifi_ssid,
                docs_host: CAPTIVE_PORTAL_HOST,
            });
        #[cfg(not(feature = "wifi-auto"))]
        let local_docs = None;
        let mut ticks_to_battery: u8 = 0;
        let mut activity = screen::CardActivityTracker::<8>::new();
        let mut notice_until_ms =
            identity_startup_notice.map(|_| embassy_time::Instant::now().as_millis() + 5_000);
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
            let content = screen::ScreenContent {
                cards: &cards,
                local_docs: local_docs.as_ref(),
            };
            #[cfg(feature = "wifi-auto")]
            let menu_ap_ssid = active_ap_ssid.as_deref();
            #[cfg(feature = "wifi-auto")]
            let interface_menu_details = build_interface_menu_details(
                ui_state.selected_card(content.cards),
                &snapshots,
                usb_status,
                &wifi_config,
                menu_ap_ssid,
            );
            #[cfg(not(feature = "wifi-auto"))]
            let interface_menu_details = {
                let mut details = screen::InterfaceMenuDetails::empty();
                add_reactor_pressure(&mut details, ui_state.selected_card(content.cards));
                details
            };
            ui_state.sync(content);
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
                screen::render(
                    &mut display,
                    screen::RenderFrame {
                        content,
                        battery: battery_state,
                        state: &ui_state,
                        interface_menu_details: &interface_menu_details,
                        animation_ms: now_ms,
                    },
                );
                B::flush(&mut display);
            }
            if settle_after_draw {
                Timer::after(Duration::from_millis(screen::COALESCE_MS)).await;
                settle_after_draw = false;
            }

            match select3(
                BUTTON_EVENTS.receive(),
                render_tick.next(),
                INTERFACE_STORE.changed(),
            )
            .await
            {
                Either3::Third(()) => {
                    settle_after_draw = true;
                }
                Either3::Second(()) => {
                    ticks_to_battery = ticks_to_battery.saturating_sub(1);
                }
                Either3::First(event) => {
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
                    match ui_state.handle_input(event, content) {
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
                            if let Some(card) = ui_state.selected_card(content.cards) {
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
                                if card.id() == usb_status.id() {
                                    show_toggle_notice(usb_status.is_enabled());
                                    usb_status.toggle_enabled();
                                    handled = true;
                                }
                                if !handled && card.id() == lora_status.id() {
                                    show_toggle_notice(lora_status.is_enabled());
                                    lora_status.toggle_enabled();
                                    handled = true;
                                }
                                if !handled {
                                    if let Some(status) = wifi_status.as_ref() {
                                        if card.id() == status.id() {
                                            show_toggle_notice(status.is_enabled());
                                            status.toggle_enabled();
                                            handled = true;
                                        }
                                    }
                                }
                                if !handled && Some(card.id()) == espnow_card_id {
                                    if let Some(status) = espnow_card_status {
                                        show_toggle_notice(status.is_enabled());
                                        status.toggle_enabled();
                                        handled = true;
                                    }
                                }
                                if !handled {
                                    if let (Some(tcp), Some(tcp_id)) = (tcp_status, tcp_id) {
                                        if card.id() == tcp_id {
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
                                if !handled && card.id() == BLE_SUPERVISOR_ID {
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
    let ble_connector = esp_radio::ble::controller::BleConnector::new(
        bluetooth,
        esp_radio::ble::Config::default()
            .with_task_stack_size(4096)
            .with_max_connections(BLE_PEER_CAPACITY as u8),
    )
    .expect("ble connector");

    #[cfg(all(feature = "bluetooth-auto", not(feature = "wifi-auto")))]
    {
        let _ = (wifi, tcp, has_wifi);
        if let Some((identity, fleet)) = ble {
            spawner.spawn(
                ble_task(spawner, ble_connector, mac_octets, identity, fleet)
                    .expect("Bluetooth task fits"),
            );
        }
        render.await;
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
            (Some((wifi, wifi_fleet)), Some((tcp, tcp_seam))) => {
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
            (Some((wifi, wifi_fleet)), None) => {
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
                let ble_connector = esp_radio::ble::controller::BleConnector::new(
                    bluetooth,
                    esp_radio::ble::Config::default()
                        .with_task_stack_size(4096)
                        .with_max_connections(BLE_PEER_CAPACITY as u8),
                )
                .expect("ble connector");
                if let Some((identity, fleet)) = ble {
                    spawner.spawn(
                        ble_task(spawner, ble_connector, mac_octets, identity, fleet)
                            .expect("Bluetooth task fits"),
                    );
                }
                match (wifi, tcp) {
                    (Some((wifi, wifi_fleet)), Some((tcp, tcp_seam))) => {
                        join(
                            join(join(lora_run, espnow_run), tcp.run(tcp_seam)),
                            join(
                                wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                                render,
                            ),
                        )
                        .await;
                    }
                    (Some((wifi, wifi_fleet)), None) => {
                        join(
                            join(lora_run, espnow_run),
                            join(
                                wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                                render,
                            ),
                        )
                        .await;
                    }
                    (None, _) => {
                        join(join(lora_run, espnow_run), render).await;
                    }
                }
            }
            RadioMode::AccessPoint => {
                let _ = (bluetooth, ble);
                match (wifi, tcp) {
                    (Some((wifi, wifi_fleet)), Some((tcp, tcp_seam))) => {
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
                    (Some((wifi, wifi_fleet)), None) => {
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

#[embassy_executor::task]
async fn reactor_task(node: &'static mut S3Node) {
    node.run_reactor_with_interface_store(&INTERFACE_STORE)
        .await
}

#[cfg(feature = "bluetooth-auto")]
#[embassy_executor::task]
async fn ble_task(
    spawner: Spawner,
    connector: esp_radio::ble::controller::BleConnector<'static>,
    mac: [u8; 6],
    identity: BleIdentity,
    fleet: S3BleFleet,
) {
    crate::bluetooth_auto::run(connector, mac, identity, fleet, &BLE_SHARED, spawner).await
}
