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
    let S3ManifoldHardware {
        cpu_control,
        software_interrupt,
        timebase,
        rtc,
    } = hardware.manifold;
    #[cfg(feature = "wifi-auto")]
    let (wifi_config, wifi_config_source) = hopspot_wifi_config();
    #[cfg(feature = "wifi-auto")]
    let station_configured = wifi_config.has_station();
    #[cfg(not(feature = "wifi-auto"))]
    let station_configured = false;
    let radio_mode = boot_radio_mode(station_configured);
    #[cfg(feature = "wifi-auto")]
    log::info!(
        "wifi-config source={wifi_config_source:?} station={} ssid_len={} password_len={} tcp={}",
        station_configured,
        wifi_config.ssid.len(),
        wifi_config.password.len(),
        wifi_config.tcp_client.is_some()
    );

    let usb_status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new(B::USB_INTERFACE_ID, ConnectionState::Initializing)
    );
    let usb_id = usb_status.id();
    let (usb_rx, usb_tx) = UsbSerialJtag::new(usb_device).into_async().split();

    let mac = base_mac_address();
    let mut mac_octets = [0u8; 6];
    mac_octets.copy_from_slice(&mac.as_bytes()[..6]);

    let mut manifold_lanes = ManifoldLanes::new();

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
    let lora_spectrum: &'static LoRaSpectrumStatus =
        mk_static!(LoRaSpectrumStatus, LoRaSpectrumStatus::new());
    #[cfg(feature = "lora")]
    let lora_tx_queue = crate::storage::allocate_lora_tx_queue();
    #[cfg(feature = "lora")]
    let lora = match LoRaInterface::new(LoRaInterfaceInput {
        radio: lora_radio,
        profile: lora_profile,
        airtime_policy: AirtimePolicy::Regional,
        tx_queue: lora_tx_queue,
        control: &LORA_CONTROL,
        status: lora_status,
        spectrum: lora_spectrum,
        lifecycle: LIFECYCLE.dyn_sender(),
    }) {
        Ok(lora) => lora,
        Err(_) => panic!("the built-in LoRa profile and regional policy must be valid"),
    };

    // The Wi-Fi stack carries both the Wi-Fi Auto UDP and the TCP client, so it stands up before the
    // node moves to core 1 — activating the TCP slot is a core-0-only act.
    #[cfg(feature = "wifi-auto")]
    boot_stage(BootPhase::WifiBegin);
    #[cfg(feature = "wifi-auto")]
    let (wifi, tcp_stack, esp_now) = build_wifi(
        &spawner,
        wifi_hardware,
        mac_octets,
        &wifi_config,
        radio_mode == RadioMode::AccessPoint,
    );
    #[cfg(feature = "wifi-auto")]
    boot_stage(BootPhase::WifiReady);
    #[cfg(feature = "wifi-auto")]
    log::info!(
        "Wi-Fi initialized station={} network_stack={}",
        wifi.is_some(),
        tcp_stack.is_some()
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
    let destination_secret = node_identity.into_destination_secret();
    let destinations = personal_hopspot_core::HopspotDestinationSet::new(
        destination_secret,
        B::ANNOUNCE_APP_DATA,
        B::NODE_ANNOUNCE_APP_DATA,
    );
    let destination_hashes = destinations
        .destination_hashes()
        .expect("the hopspot destination names are valid");
    let self_destination = destination_hashes.delivery;
    let node_page_destination = destination_hashes.node_page;
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

    #[cfg(feature = "wifi-auto")]
    boot_stage(BootPhase::TcpBegin);
    #[cfg(feature = "wifi-auto")]
    let tcp_built = tcp_stack.and_then(|stack| {
        wifi_config
            .tcp_client
            .as_ref()
            .and_then(|tcp_client| build_tcp(stack, tcp_client))
    });
    #[cfg(feature = "wifi-auto")]
    boot_stage(BootPhase::TcpReady);
    #[cfg(not(feature = "wifi-auto"))]
    let tcp_built: Option<(
        TcpClient<'static>,
        &'static EmbassyInterfaceStatus,
        InterfaceId,
    )> = None;
    let tcp_status = tcp_built.as_ref().map(|(_, status, _)| *status);
    let tcp_id = tcp_built.as_ref().map(|(_, _, id)| *id);

    let recipe = PrnsNodeRecipe {
        transport_identity: Some(transport_secret),
        pre_configured_destinations: destinations.into_preconfigured_destinations(),
        app_state: (),
        storage: EngineStorageType::default(),
        request_endpoints: screen::node_pages::NodePageRoutes,
        interfaces: personal_rns::runtime::ManuallyAttached,
        persistence: crate::persistence::s3(),
        on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
    };

    #[cfg(feature = "lora")]
    let lora_cfg = lora.descriptor();
    #[cfg(feature = "esp-now")]
    let espnow_cfg = espnow.as_ref().map(|e| e.descriptor());
    let tcp_cfg = tcp_built.as_ref().map(|(t, _, _)| t.descriptor());
    let has_wifi = wifi.is_some();

    let usb_lane = manifold_lanes
        .claim_interface(&USB_MANIFOLD_LANE, device_descriptor(usb_id))
        .expect("USB lane is available");
    let tcp_lane = tcp_cfg.map(|descriptor| {
        manifold_lanes
            .claim_interface(&TCP_MANIFOLD_LANE, descriptor)
            .expect("TCP lane is available")
    });
    #[cfg(feature = "wifi-auto")]
    let wifi_supervisor_lane = has_wifi.then(|| {
        manifold_lanes
            .claim_supervisor(&WIFI_MANIFOLD_LANE, WIFI_SUPERVISOR_ID, &OUTBOUND_WAKE)
            .expect("Wi-Fi supervisor lane is available")
    });
    #[cfg(feature = "lora")]
    let lora_lane = manifold_lanes
        .claim_interface(&LORA_MANIFOLD_LANE, lora_cfg)
        .expect("LoRa lane is available");
    #[cfg(feature = "bluetooth-auto")]
    let ble_supervisor_lane = (radio_mode == RadioMode::Ble && ble_identity.is_some()).then(|| {
        manifold_lanes
            .claim_supervisor(&BLE_MANIFOLD_LANE, BLE_SUPERVISOR_ID, &BLE_OUTBOUND_WAKE)
            .expect("Bluetooth supervisor lane is available")
    });
    #[cfg(feature = "esp-now")]
    let espnow_lane = espnow_cfg.map(|descriptor| {
        manifold_lanes
            .claim_interface(&ESPNOW_MANIFOLD_LANE, descriptor)
            .expect("ESP-NOW lane is available")
    });

    let handle: Handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let manifold_wiring = manifold_lanes.into_manifold_wiring(
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new_with_timebase(timebase, hardware_entropy as fn(&mut [u8]));

    let core1_stack = mk_static!(CpuStack<CORE1_STACK_BYTES>, CpuStack::new());
    boot_stage(BootPhase::CoreOneStartBegin);
    esp_rtos::start_second_core(cpu_control, software_interrupt, core1_stack, move || {
        static NODE: StaticCell<S3Node> = StaticCell::new();
        let (node, persistence) =
            PrnsNode::init_static_with_persistence(&NODE, recipe, manifold_wiring, host);
        static PERSISTENCE: StaticCell<crate::persistence::S3Persistence> = StaticCell::new();
        let persistence = PERSISTENCE.init(persistence);

        static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
        boot_stage(BootPhase::CoreOneExecutorReady);
        EXECUTOR
            .init(esp_rtos::embassy::Executor::new())
            .run(|spawner| {
                spawner.spawn(manifold_task(node, persistence).expect("manifold task fits"));
                spawner.spawn(core_one_liveness_task().expect("core-one liveness task fits"));
            })
    });
    boot_stage(BootPhase::CoreOneStartReady);

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
        let fleet: S3WifiFleet = lane.into_fleet(NOTIFY.sender(), LIFECYCLE.sender());
        (interface, fleet)
    });
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
    #[cfg(feature = "wifi-auto")]
    if let Some((interface, fleet)) = wifi {
        let data_buf: &'static mut [u8] = alloc::vec![0u8; wifi_auto_contract::HARDWARE_MTU].leak();
        let secondary_data_buf: &'static mut [u8] =
            alloc::vec![0u8; wifi_auto_contract::HARDWARE_MTU].leak();
        spawner.spawn(
            wifi_task(interface, fleet, data_buf, secondary_data_buf).expect("Wi-Fi task fits"),
        );
    }

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
        boot_stage(BootPhase::DisplayRuntimeBegin);
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
        let mut persistence_notice_visible = false;
        let mut first_render_pending = true;
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
            #[cfg(feature = "wifi-auto")]
            let tcp_card_config = wifi_config.tcp_client.as_ref();
            #[cfg(not(feature = "wifi-auto"))]
            let tcp_card_config: Option<&HopspotTcpClientConfig> = None;
            let mut cards = build_cards(
                &snapshots,
                usb_status.id(),
                wifi_id,
                tcp_id,
                tcp_card_config,
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
                lora_spectrum,
                &wifi_config,
                menu_ap_ssid,
            );
            #[cfg(not(feature = "wifi-auto"))]
            let interface_menu_details = {
                let mut details = screen::InterfaceMenuDetails::empty();
                add_lora_spectrum(
                    &mut details,
                    ui_state.selected_card(content.cards),
                    lora_spectrum,
                );
                add_manifold_pressure(&mut details, ui_state.selected_card(content.cards));
                details
            };
            ui_state.sync(content);
            let state_not_saved = crate::persistence::state_not_saved();
            if state_not_saved {
                ui_state.show_notice(screen::UiNotice::StateNotSaved);
                notice_until_ms = None;
                persistence_notice_visible = true;
            } else if persistence_notice_visible {
                ui_state.clear_notice();
                persistence_notice_visible = false;
            }
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
                if first_render_pending {
                    boot_stage(BootPhase::DisplayFirstRenderBegin);
                }
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
                if first_render_pending {
                    boot_stage(BootPhase::DisplayFirstRenderComplete);
                    first_render_pending = false;
                }
            } else if first_render_pending {
                boot_stage(BootPhase::DisplayFirstRenderUnavailable);
                first_render_pending = false;
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
                            boot_stage(BootPhase::AnnounceBegin);
                            ui_state.show_notice(screen::UiNotice::Announcing);
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                            let delivery_queued =
                                handle.issue(PrnsCommand::AnnounceNow(AnnounceNow {
                                    destination: self_destination,
                                    target: AnnounceTarget::AllInterfaces,
                                    app_data: AnnounceAppData::Registered,
                                }));
                            boot_stage(BootPhase::AnnounceDeliveryIssueReturned);
                            log::info!(
                                "announce-ui destination=delivery queued={}",
                                delivery_queued.is_some()
                            );
                            let node_queued = handle.issue(PrnsCommand::AnnounceNow(AnnounceNow {
                                destination: node_page_destination,
                                target: AnnounceTarget::AllInterfaces,
                                app_data: AnnounceAppData::Registered,
                            }));
                            boot_stage(BootPhase::AnnounceNodeIssueReturned);
                            log::info!(
                                "announce-ui destination=node queued={}",
                                node_queued.is_some()
                            );
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
    boot_stage(BootPhase::BluetoothBegin);
    #[cfg(all(feature = "bluetooth-auto", not(feature = "wifi-auto")))]
    let ble_connector = esp_radio::ble::controller::BleConnector::new(
        bluetooth,
        esp_radio::ble::Config::default()
            .with_task_stack_size(4096)
            .with_max_connections(BLE_PEER_CAPACITY as u8),
    )
    .expect("ble connector");
    #[cfg(all(feature = "bluetooth-auto", not(feature = "wifi-auto")))]
    boot_stage(BootPhase::BluetoothReady);

    spawner.spawn(watchdog_task(rtc.rwdt).expect("watchdog task fits"));

    #[cfg(all(feature = "bluetooth-auto", not(feature = "wifi-auto")))]
    {
        let _ = (tcp, has_wifi);
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
        match tcp {
            Some((tcp, tcp_seam)) => {
                join(join(join(lora_run, espnow_run), tcp.run(tcp_seam)), render).await;
            }
            None => {
                join(join(lora_run, espnow_run), render).await;
            }
        }
    }
    #[cfg(all(feature = "bluetooth-auto", feature = "wifi-auto"))]
    {
        spawner.spawn(lora_task(lora, lora_seam).expect("LoRa task fits"));
        if let Some((interface, seam)) = espnow {
            spawner.spawn(espnow_task(interface, seam).expect("ESP-NOW task fits"));
        }
        if let Some((interface, seam)) = tcp {
            spawner.spawn(tcp_task(interface, seam).expect("TCP task fits"));
        }
        match radio_mode {
            RadioMode::Ble => {
                boot_stage(BootPhase::BluetoothBegin);
                let ble_connector = esp_radio::ble::controller::BleConnector::new(
                    bluetooth,
                    esp_radio::ble::Config::default()
                        .with_task_stack_size(4096)
                        .with_max_connections(BLE_PEER_CAPACITY as u8),
                )
                .expect("ble connector");
                boot_stage(BootPhase::BluetoothReady);
                if let Some((identity, fleet)) = ble {
                    spawner.spawn(
                        ble_task(spawner, ble_connector, mac_octets, identity, fleet)
                            .expect("Bluetooth task fits"),
                    );
                }
            }
            RadioMode::AccessPoint => {
                let _ = (bluetooth, ble);
            }
        }
        render.await;
    }
}

#[cfg(feature = "lora")]
#[embassy_executor::task]
async fn lora_task(interface: S3LoraInterface, seam: S3LoraSeam) {
    interface.run(seam).await
}

#[cfg(feature = "esp-now")]
#[embassy_executor::task]
async fn espnow_task(interface: S3EspNowInterface, seam: S3EspNowSeam) {
    interface.run(seam).await
}

#[embassy_executor::task]
async fn tcp_task(interface: TcpClient<'static>, seam: S3TcpSeam) {
    interface.run(seam).await
}

#[cfg(feature = "wifi-auto")]
#[embassy_executor::task]
async fn wifi_task(
    interface: AutoWifi<'static, MEMBERS>,
    fleet: S3WifiFleet,
    data_buf: &'static mut [u8],
    secondary_data_buf: &'static mut [u8],
) {
    interface.run(fleet, data_buf, secondary_data_buf).await
}

#[embassy_executor::task]
async fn manifold_task(
    node: &'static mut S3Node,
    persistence: &'static mut crate::persistence::S3Persistence,
) {
    boot_stage(BootPhase::PersistenceRestoreBegin);
    let _ = node.restore_embedded_persistence(persistence).await;
    boot_stage(BootPhase::PersistenceRestoreComplete);
    node.run_manifold_with_persistence_and_interface_store(&INTERFACE_STORE, persistence)
        .await
}

#[embassy_executor::task]
async fn watchdog_task(mut watchdog: esp_hal::rtc_cntl::Rwdt) -> ! {
    watchdog.enable();
    watchdog.set_timeout(
        esp_hal::rtc_cntl::RwdtStage::Stage0,
        esp_hal::time::Duration::from_secs(15),
    );
    watchdog.set_stage_action(
        esp_hal::rtc_cntl::RwdtStage::Stage0,
        esp_hal::rtc_cntl::RwdtStageAction::ResetSystem,
    );
    watchdog.feed();
    boot_stage(BootPhase::WatchdogReady);
    let mut last_core_one_heartbeat = CORE_ONE_HEARTBEAT.load(Ordering::Relaxed);
    let mut core_one_stalled_ticks = 0u32;
    loop {
        Timer::after(Duration::from_secs(1)).await;
        let core_one_heartbeat = CORE_ONE_HEARTBEAT.load(Ordering::Relaxed);
        if core_one_heartbeat != last_core_one_heartbeat {
            last_core_one_heartbeat = core_one_heartbeat;
            core_one_stalled_ticks = 0;
            watchdog.feed();
        } else {
            core_one_stalled_ticks = core_one_stalled_ticks.saturating_add(1);
            if core_one_stalled_ticks == 2 {
                log::warn!("watchdog: core1 heartbeat missing");
            }
        }
    }
}

#[embassy_executor::task]
async fn core_one_liveness_task() -> ! {
    loop {
        Timer::after(Duration::from_secs(1)).await;
        CORE_ONE_HEARTBEAT.fetch_add(1, Ordering::Relaxed);
    }
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
