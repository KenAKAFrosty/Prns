use super::*;
use personal_hopspot_core::display::{
    DisplayBlankReason, DisplayDuration, DisplayVisibility, MonotonicMillis, PresentationUrgency,
};
use personal_rns::remote_control::{
    RemoteControlInitialAccess, RemoteControlSelfAnnouncement, RemoteControlService,
};

fn display_now() -> MonotonicMillis {
    MonotonicMillis::new(embassy_time::Instant::now().as_millis())
}

const NOTICE_DURATION: DisplayDuration = match DisplayDuration::from_millis(NOTICE_MS) {
    Ok(duration) => duration,
    Err(_) => panic!("the notice duration is nonzero"),
};
const STARTUP_NOTICE_DURATION: DisplayDuration = match DisplayDuration::from_millis(5_000) {
    Ok(duration) => duration,
    Err(_) => panic!("the startup notice duration is nonzero"),
};

fn show_notice(
    state: &mut screen::UiState,
    timer: &mut screen::PresentedNoticeTimer,
    notice: screen::UiNotice,
    duration: DisplayDuration,
) {
    state.show_notice(notice);
    timer.stage(notice, duration);
}

pub(crate) async fn run<B: Esp32S3Board>(spawner: Spawner)
where
    B::Display: 'static,
    <B::Display as S3BoardDisplay>::Runtime: 'static,
    B::Battery: 'static,
    B::Gnss: 'static,
{
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);
    let bringup = B::bringup(p).await;
    // Pin into esp_alloc's global external heap, not `PsramAlloc`. On Heltec V4-R8, `PsramAlloc` is
    // the private bump that `reinit_private_psram_heap` resets inside `run_core` before the LoRa
    // queue lands — pinning here would place the live future (OLED/I2C state) in that window and
    // get overwritten, which zeroed I2C Config.frequency after radio bring-up.
    allocator_api2::boxed::Box::pin_in(run_core::<B>(spawner, bringup), esp_alloc::ExternalMemory)
        .await;
}

#[allow(clippy::too_many_lines)]
pub(super) async fn run_core<B: Esp32S3Board>(
    spawner: Spawner,
    hardware: S3BoardHardware<B::Display, B::Battery, B::Gnss>,
) where
    B::Display: 'static,
    <B::Display as S3BoardDisplay>::Runtime: 'static,
    B::Battery: 'static,
    B::Gnss: 'static,
{
    let BoardFace {
        display: board_display,
        battery,
        button,
    } = hardware.face;
    let mut battery_source = battery;
    let gnss = hardware.gnss;
    let S3InterfaceHardware {
        usb_device,
        #[cfg(feature = "lora")]
        lora_radio,
        wifi: wifi_hardware,
        bluetooth,
    } = hardware.interface_hardware;
    let S3ManifoldHardware {
        cpu_control,
        software_interrupt,
        timebase,
        rtc,
    } = hardware.manifold;
    let (wifi_config, wifi_config_source) = hopspot_wifi_config();
    let station_configured = wifi_config.has_station();
    let radio_mode = boot_radio_mode(station_configured);
    log::info!(
        "wifi-config source={wifi_config_source:?} station={} ssid_len={} password_len={} tcp={}",
        station_configured,
        wifi_config.ssid.len(),
        wifi_config.password.len(),
        wifi_config.tcp_client.is_some()
    );

    // Defer claiming USB-JTAG until after Wi-Fi bring-up so boot logs stay visible through radio init.
    let usb_status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new_accounted(B::USB_INTERFACE_ID, ConnectionState::Initializing,)
    );
    let usb_id = usb_status.id();
    let mac = base_mac_address();
    let mut mac_octets = [0u8; 6];
    mac_octets.copy_from_slice(&mac.as_bytes()[..6]);

    let mut manifold_lanes = ManifoldLanes::new();

    static FLASH: StaticCell<Mutex<CriticalSectionRawMutex, crate::flash::EspRomFlash>> =
        StaticCell::new();
    let flash = FLASH.init(Mutex::new(crate::flash::EspRomFlash::new(
        B::FLASH_LAYOUT.flash_capacity,
    )));
    let shared_flash = SharedNorFlash::new(flash, B::FLASH_LAYOUT.flash_capacity);
    #[cfg(feature = "lora")]
    let mut lora_profile_store =
        screen::RadioProfileStore::new(shared_flash, B::FLASH_LAYOUT.radio_profile_pages);
    #[cfg(feature = "lora")]
    let loaded_lora_profile = match lora_profile_store
        .load(boot_lora_profile(B::MAX_TX_POWER_DBM))
        .await
    {
        Ok(loaded) => screen::LoadedRadioProfile {
            profile: loaded.profile.with_tx_power_at_most(B::MAX_TX_POWER_DBM),
            follows_default: loaded.follows_default,
            notice: loaded.notice,
        },
        Err(error) => {
            log::error!("LoRa profile restore failed: {error:?}");
            screen::LoadedRadioProfile {
                profile: boot_lora_profile(B::MAX_TX_POWER_DBM),
                follows_default: true,
                notice: Some(screen::RadioProfileLoadNotice::Reset),
            }
        }
    };
    #[cfg(feature = "lora")]
    let lora_profile = loaded_lora_profile.profile;
    #[cfg(feature = "lora")]
    let profile_startup_notice = loaded_lora_profile.notice.map(|notice| match notice {
        screen::RadioProfileLoadNotice::Recovered => screen::UiNotice::ProfileRecovered,
        screen::RadioProfileLoadNotice::Reset => screen::UiNotice::ProfileReset,
    });
    #[cfg(not(feature = "lora"))]
    let profile_startup_notice: Option<screen::UiNotice> = None;
    #[cfg(feature = "lora")]
    let lora_id = LoRaInterface::<LoraRadio>::interface_id(&lora_profile);
    #[cfg(feature = "lora")]
    let lora_status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new_accounted(lora_id, ConnectionState::Initializing)
    );
    #[cfg(feature = "lora")]
    let lora_spectrum: &'static LoRaSpectrumStatus =
        mk_static!(LoRaSpectrumStatus, LoRaSpectrumStatus::new());
    // The Option shims mirror ESP-NOW's: boards without an SX1262 keep every downstream card,
    // toggle, and sleep path compiling against `None` instead of forking the render loop.
    #[cfg(feature = "lora")]
    let (lora_card_id, lora_card_status): (
        Option<InterfaceId>,
        Option<&'static EmbassyInterfaceStatus>,
    ) = (Some(lora_id), Some(lora_status));
    #[cfg(not(feature = "lora"))]
    let (lora_card_id, lora_card_status): (
        Option<InterfaceId>,
        Option<&'static EmbassyInterfaceStatus>,
    ) = (None, None);
    // Reclaim the private R8 probe allocation before placing the live LoRa queue in PSRAM.
    // This is a no-op on boards whose PSRAM belongs to the global heap.
    #[cfg(feature = "lora")]
    crate::storage::reinit_private_psram_heap();
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

    // Reconstruct identities before radio bring-up, on a temporary RTOS task stack. Curve25519's
    // stack high-water mark is too large for the guarded core-0 main stack, and doing the work here
    // also keeps it out of the live Wi-Fi/BLE scheduling window.
    let (node_bootstrap, remote_control_bootstrap, ble_bootstrap, destination_hashes) =
        crate::identity::bootstrap_s3_identities(B::FLASH_LAYOUT.into()).await;
    let remote_control_bootstrap =
        remote_control_bootstrap.expect("RemoteControl identity bootstrap failed");
    crate::identity::log_persistence("node", node_bootstrap.persistence());
    crate::identity::log_persistence("Bluetooth", ble_bootstrap.persistence());

    // The Wi-Fi stack carries both the Wi-Fi Auto UDP and the TCP client, so it stands up before the
    // node moves to core 1 — activating the TCP slot is a core-0-only act.
    boot_stage(BootPhase::WifiBegin);
    let (wifi, tcp_stack, esp_now) = build_wifi(
        &spawner,
        wifi_hardware,
        mac_octets,
        &wifi_config,
        radio_mode == RadioMode::AccessPoint,
    );
    boot_stage(BootPhase::WifiReady);
    log::info!(
        "Wi-Fi initialized station={} network_stack={}",
        wifi.is_some(),
        tcp_stack.is_some()
    );
    let identity_startup_notice =
        crate::identity::startup_notice(node_bootstrap.persistence(), ble_bootstrap.persistence());
    let node_identity = node_bootstrap.into_identity();
    let transport_secret = node_identity.transport_secret();
    let destination_secret = node_identity.into_destination_secret();
    let (remote_control_identity_secrets, _remote_control_identity_origins) =
        remote_control_bootstrap.into_parts();
    let remote_control = RemoteControlService::new(
        remote_control_identity_secrets,
        RemoteControlInitialAccess::Nobody,
        RemoteControlSelfAnnouncement::Destination(destination_hashes.node_page),
    );
    let destinations = personal_hopspot_core::HopspotDestinationSet::new(
        destination_secret,
        B::ANNOUNCE_APP_DATA,
        B::NODE_ANNOUNCE_APP_DATA,
    );
    let node_page_destination = destination_hashes.node_page;
    let ble_identity = Some(ble_bootstrap.into_identity());

    let espnow_status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new_accounted(
            espnow_core::interface_id(),
            ConnectionState::Initializing,
        )
    );
    let espnow = esp_now.map(|radio| {
        EspNowInterface::new(
            EspNowAdapter::new(radio),
            espnow_channel_policy(station_configured),
            ESPNOW_PHY.bitrate,
            espnow_status,
        )
    });

    boot_stage(BootPhase::TcpBegin);
    let tcp_built = tcp_stack.and_then(|stack| {
        wifi_config
            .tcp_client
            .as_ref()
            .and_then(|tcp_client| build_tcp(stack, tcp_client))
    });
    boot_stage(BootPhase::TcpReady);
    let tcp_status = tcp_built.as_ref().map(|(_, status, _)| *status);
    let tcp_id = tcp_built.as_ref().map(|(_, _, id)| *id);

    let recipe = PrnsNodeRecipe {
        transport_identity: Some(transport_secret),
        remote_control,
        pre_configured_destinations: destinations.into_preconfigured_destinations(),
        app_state: (),
        storage: EngineStorageType::default(),
        request_endpoints: screen::node_pages::NodePageRoutes,
        interfaces: personal_rns::runtime::ManuallyAttached,
        persistence: crate::persistence::s3(shared_flash, B::FLASH_LAYOUT.journal),
        on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
    };

    #[cfg(feature = "lora")]
    let lora_cfg = lora.descriptor();
    let espnow_cfg = espnow.as_ref().map(|e| e.descriptor());
    let tcp_cfg = tcp_built.as_ref().map(|(t, _, _)| t.descriptor());
    let has_wifi = wifi.is_some();

    let usb_outbound = crate::storage::allocate_manifold_outbound::<EMBEDDED_MAX_WIRE_FRAME_LEN>(
        OUTBOUND_BURST_DEPTH,
    );
    let usb_lane = manifold_lanes
        .claim_accounted_interface_with_outbound_buffer(
            &USB_MANIFOLD_LANE,
            device_descriptor(usb_id),
            usb_outbound,
            usb_status,
        )
        .expect("USB lane is available");
    let tcp_lane = tcp_cfg.zip(tcp_status).map(|(descriptor, status)| {
        let outbound = crate::storage::allocate_manifold_outbound::<EMBEDDED_MAX_WIRE_FRAME_LEN>(
            OUTBOUND_BURST_DEPTH,
        );
        manifold_lanes
            .claim_accounted_interface_with_outbound_buffer(
                &TCP_MANIFOLD_LANE,
                descriptor,
                outbound,
                status,
            )
            .expect("TCP lane is available")
    });
    let wifi_supervisor_lane = has_wifi.then(|| {
        let outbound = crate::storage::allocate_manifold_outbound::<
            { wifi_auto_contract::HARDWARE_MTU },
        >(OUTBOUND_BURST_DEPTH);
        manifold_lanes
            .claim_supervisor_with_outbound_buffer(
                &WIFI_MANIFOLD_LANE,
                WIFI_SUPERVISOR_ID,
                &OUTBOUND_WAKE,
                outbound,
            )
            .expect("Wi-Fi supervisor lane is available")
    });
    #[cfg(feature = "lora")]
    let lora_outbound =
        crate::storage::allocate_manifold_outbound::<LORA_MAX_PAYLOAD>(OUTBOUND_BURST_DEPTH);
    #[cfg(feature = "lora")]
    let lora_lane = manifold_lanes
        .claim_accounted_interface_with_outbound_buffer(
            &LORA_MANIFOLD_LANE,
            lora_cfg,
            lora_outbound,
            lora_status,
        )
        .expect("LoRa lane is available");
    let ble_supervisor_lane = (radio_mode == RadioMode::Ble && ble_identity.is_some()).then(|| {
        let outbound =
            crate::storage::allocate_manifold_outbound::<BLE_HW_MTU>(OUTBOUND_BURST_DEPTH);
        manifold_lanes
            .claim_supervisor_with_outbound_buffer(
                &BLE_MANIFOLD_LANE,
                BLE_SUPERVISOR_ID,
                &BLE_OUTBOUND_WAKE,
                outbound,
            )
            .expect("Bluetooth supervisor lane is available")
    });
    let espnow_lane = espnow_cfg.map(|descriptor| {
        let outbound =
            crate::storage::allocate_manifold_outbound::<ESP_NOW_V2_AIR_MTU>(OUTBOUND_BURST_DEPTH);
        manifold_lanes
            .claim_accounted_interface_with_outbound_buffer(
                &ESPNOW_MANIFOLD_LANE,
                descriptor,
                outbound,
                espnow_status,
            )
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
        node.set_protocol_policy(personal_hopspot_core::EMBEDDED_HOPSPOT_PROTOCOL_POLICY);
        static PERSISTENCE: StaticCell<crate::persistence::S3Persistence> = StaticCell::new();
        let persistence = PERSISTENCE.init(persistence);

        static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
        boot_stage(BootPhase::CoreOneExecutorReady);
        EXECUTOR
            .init(esp_rtos::embassy::Executor::new())
            .run(|spawner| {
                let run = crate::storage::allocate_psram(manifold_run(node, persistence));
                let run: core::pin::Pin<&'static mut dyn core::future::Future<Output = ()>> =
                    // SAFETY: `allocate_psram` leaks this allocation, so it cannot move or be freed.
                    unsafe { core::pin::Pin::new_unchecked(run) };
                spawner.spawn(manifold_task(run).expect("manifold task fits"));
                spawner.spawn(core_one_liveness_task().expect("core-one liveness task fits"));
            })
    });
    boot_stage(BootPhase::CoreOneStartReady);

    let (usb_rx, usb_tx) = UsbSerialJtag::new(usb_device).into_async().split();
    let usb_seam = usb_lane.into_seam(NOTIFY.sender(), hardware_entropy);
    spawner.spawn(usb_device_task(usb_rx, usb_tx, usb_seam, usb_status).expect("usb task fits"));

    #[cfg(feature = "lora")]
    let lora_seam = lora_lane.into_seam(NOTIFY.sender(), hardware_entropy);

    let espnow = espnow.zip(espnow_lane).map(|(interface, lane)| {
        let seam = lane.into_seam(NOTIFY.sender(), hardware_entropy);
        (interface, seam)
    });

    let tcp = tcp_built.zip(tcp_lane).map(|((tcp, _, _), lane)| {
        let seam = lane.into_seam(NOTIFY.sender(), hardware_entropy);
        (tcp, seam)
    });

    let wifi = wifi.zip(wifi_supervisor_lane).map(|(interface, lane)| {
        let fleet: S3WifiFleet = lane.into_fleet(NOTIFY.sender(), LIFECYCLE.sender());
        (interface, fleet)
    });
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
    if let Some((interface, fleet)) = wifi {
        let data_buf: &'static mut [u8] = alloc::vec![0u8; wifi_auto_contract::HARDWARE_MTU].leak();
        let secondary_data_buf: &'static mut [u8] =
            alloc::vec![0u8; wifi_auto_contract::HARDWARE_MTU].leak();
        // Construct the large dual-segment state machine in PSRAM before Embassy measures its
        // task arguments. Passing only a pinned trait-object pointer keeps the task slot small and
        // leaves internal SRAM available to the closed-source radio driver.
        let run =
            crate::storage::allocate_psram(interface.run(fleet, data_buf, secondary_data_buf));
        let run: core::pin::Pin<&'static mut dyn core::future::Future<Output = ()>> =
            // SAFETY: `allocate_psram` leaks this allocation, so it cannot move or be freed.
            unsafe { core::pin::Pin::new_unchecked(run) };
        spawner.spawn(wifi_task(run).expect("Wi-Fi task fits"));
    }

    let espnow_card_id = espnow.as_ref().map(|(interface, _)| interface.id());
    let espnow_card_status = espnow_card_id.map(|_| espnow_status);

    let render = async move {
        boot_stage(BootPhase::DisplayRuntimeBegin);
        let mut display = board_display.into_runtime(display_now());
        let access_point = match radio_mode {
            RadioMode::AccessPoint => screen::AccessPointState::Active,
            RadioMode::Ble => screen::AccessPointState::Inactive,
        };
        let mut ui_state = screen::UiState::new(screen::UiConfiguration {
            storage_limits: <EngineStorageType as StorageLayout>::LIMITS,
            user_blanking: display.user_blanking(),
            access_point,
            shared_instance_config_export: screen::SharedInstanceConfigExport::Unavailable,
            gnss: B::Gnss::AVAILABILITY,
        });
        let startup_notice = identity_startup_notice.or(profile_startup_notice);
        let mut pending_startup_notice = identity_startup_notice
            .is_some()
            .then_some(profile_startup_notice)
            .flatten();
        let mut notice_timer = screen::PresentedNoticeTimer::new();
        if let Some(notice) = startup_notice {
            show_notice(
                &mut ui_state,
                &mut notice_timer,
                notice,
                STARTUP_NOTICE_DURATION,
            );
        }
        #[cfg(feature = "lora")]
        let mut working_lora_profile = lora_profile;
        let mut battery_state = screen::PowerSnapshot::UNKNOWN;
        let mut sampled_battery_state = screen::PowerSnapshot::UNKNOWN;
        let mut battery_gauge = screen::BatteryGauge::lipo();
        let active_ap_ssid = (radio_mode == RadioMode::AccessPoint).then(ap_ssid);
        let local_docs = active_ap_ssid
            .as_deref()
            .map(|wifi_ssid| screen::LocalDocsAccess {
                wifi_ssid,
                docs_host: CAPTIVE_PORTAL_HOST,
            });
        let mut ticks_to_battery_sample: u8 = 0;
        let mut ticks_to_battery_display: u8 = 0;
        let mut activity = screen::CardActivityTracker::<8>::new();
        let mut render_tick = Ticker::every(RENDER_INTERVAL);
        let mut settle_after_draw = false;
        let mut persistence_notice = screen::PersistenceNotice::new();
        let mut first_render_pending = true;
        let mut first_render_started = false;
        let mut presentation_urgency = PresentationUrgency::Immediate;
        loop {
            if ticks_to_battery_sample == 0 {
                sampled_battery_state = battery_gauge.sample(&mut battery_source);
                ticks_to_battery_sample = RENDER_TICKS_PER_BATTERY_SAMPLE;
            }
            if ticks_to_battery_display == 0 {
                battery_state = sampled_battery_state;
                ticks_to_battery_display = RENDER_TICKS_PER_BATTERY_DISPLAY;
            } else {
                // The number is deliberately calm, but the plug should still react to the latest
                // two-second charging/presence observation.
                battery_state = screen::PowerSnapshot::new(
                    battery_state.battery(),
                    sampled_battery_state.external_power(),
                );
            }

            let snapshots = build_snapshots(
                usb_status,
                wifi_status.as_ref(),
                tcp_status,
                lora_card_status,
                espnow_card_status,
            );
            let tcp_card_config = wifi_config.tcp_client.as_ref();
            let mut cards = build_cards(
                &snapshots,
                usb_status.id(),
                wifi_id,
                tcp_id,
                tcp_card_config,
                wifi_status.as_ref(),
                &wifi_config,
                lora_card_id,
                espnow_card_id,
            );
            let now_ms = embassy_time::Instant::now().as_millis();
            let activity_secs = (now_ms / 1000).min(u64::from(u32::MAX)) as u32;
            activity.update(&mut cards, activity_secs);
            let content = screen::ScreenContent {
                cards: &cards,
                local_docs: local_docs.as_ref(),
            };
            let menu_ap_ssid = active_ap_ssid.as_deref();
            #[cfg(feature = "lora")]
            let interface_menu_details = build_interface_menu_details(
                ui_state.selected_card(content.cards),
                &snapshots,
                usb_status,
                lora_spectrum,
                wifi_status.as_ref(),
                &wifi_config,
                menu_ap_ssid,
            );
            #[cfg(not(feature = "lora"))]
            let interface_menu_details = build_interface_menu_details(
                ui_state.selected_card(content.cards),
                &snapshots,
                usb_status,
                wifi_status.as_ref(),
                &wifi_config,
                menu_ap_ssid,
            );
            ui_state.sync(content);
            if let Some(notice) =
                persistence_notice.observe(crate::persistence::persistence_state())
            {
                show_notice(
                    &mut ui_state,
                    &mut notice_timer,
                    notice,
                    STARTUP_NOTICE_DURATION,
                );
                presentation_urgency = PresentationUrgency::Immediate;
            }
            if let Some(owner) = notice_timer.expire(MonotonicMillis::new(now_ms)) {
                if ui_state.clear_notice_if(owner) {
                    if let Some(notice) = pending_startup_notice.take() {
                        show_notice(
                            &mut ui_state,
                            &mut notice_timer,
                            notice,
                            STARTUP_NOTICE_DURATION,
                        );
                    }
                } else {
                    pending_startup_notice = None;
                }
            }
            if let Err(error) = display.poll_blanking(MonotonicMillis::new(now_ms), display_now) {
                log::error!("display blanking failed: {error:?}");
            }
            if first_render_pending
                && !first_render_started
                && display.visibility() == DisplayVisibility::Visible
            {
                boot_stage(BootPhase::DisplayFirstRenderBegin);
                first_render_started = true;
            }
            match display
                .render_and_present(
                    screen::face_64x128::RenderInput {
                        content,
                        battery: battery_state,
                        gnss: ui_state.gnss_visible().then(B::Gnss::snapshot).flatten(),
                        state: &ui_state,
                        interface_menu_details: &interface_menu_details,
                    },
                    MonotonicMillis::new(now_ms),
                    presentation_urgency,
                    display_now,
                )
                .await
            {
                Ok(S3Presentation::Presented) if first_render_pending => {
                    boot_stage(BootPhase::DisplayFirstRenderComplete);
                    first_render_pending = false;
                    notice_timer.presentation_succeeded(ui_state.visible_notice(), display_now());
                }
                Ok(S3Presentation::Unavailable) if first_render_pending => {
                    boot_stage(BootPhase::DisplayFirstRenderUnavailable);
                    first_render_pending = false;
                }
                Ok(S3Presentation::Failed) => {
                    log::error!("display presentation failed");
                }
                Ok(S3Presentation::Presented | S3Presentation::Unchanged) => {
                    notice_timer.presentation_succeeded(ui_state.visible_notice(), display_now());
                }
                Ok(
                    S3Presentation::Unavailable
                    | S3Presentation::Withheld
                    | S3Presentation::DeferredUntil(_),
                ) => {}
                Err(error) => log::error!("display presentation state failed: {error:?}"),
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
                    presentation_urgency = PresentationUrgency::Telemetry;
                }
                Either3::Second(()) => {
                    ticks_to_battery_sample = ticks_to_battery_sample.saturating_sub(1);
                    ticks_to_battery_display = ticks_to_battery_display.saturating_sub(1);
                    presentation_urgency = PresentationUrgency::Telemetry;
                }
                Either3::First(first_event) => {
                    let mut next_event = Some(first_event);
                    for index in 0..BUTTON_EVENT_CAPACITY {
                        let Some(event) = next_event.take() else {
                            break;
                        };
                        presentation_urgency = PresentationUrgency::Immediate;
                        let now_ms = embassy_time::Instant::now().as_millis();
                        let forward_to_ui = match display
                            .button_pressed(MonotonicMillis::new(now_ms), display_now)
                        {
                            Ok(screen::display::DisplayButtonOutcome::WakeAndConsume) => {
                                if display.visibility() == DisplayVisibility::Visible {
                                    show_notice(
                                        &mut ui_state,
                                        &mut notice_timer,
                                        screen::UiNotice::Awake,
                                        NOTICE_DURATION,
                                    );
                                }
                                false
                            }
                            Ok(screen::display::DisplayButtonOutcome::ForwardToUi) => true,
                            Err(error) => {
                                log::error!("display button handling failed: {error:?}");
                                false
                            }
                        };
                        if forward_to_ui {
                            let action = ui_state.handle_input(event, content);
                            notice_timer.reconcile(ui_state.visible_notice());
                            match action {
                                screen::UiAction::BlankDisplay => {
                                    show_notice(
                                        &mut ui_state,
                                        &mut notice_timer,
                                        screen::UiNotice::DisplayOff,
                                        NOTICE_DURATION,
                                    );
                                    if let Err(error) = display.schedule_blanking(
                                        MonotonicMillis::new(now_ms.saturating_add(NOTICE_MS)),
                                        DisplayBlankReason::DisplayOnly,
                                    ) {
                                        log::error!("display-off scheduling failed: {error:?}");
                                    }
                                }
                                screen::UiAction::ToggleDisplayAutoOff => {
                                    match display.toggle_auto_off(MonotonicMillis::new(now_ms)) {
                                        Ok(auto_off) => {
                                            let notice = match auto_off {
                                                screen::display::DisplayAutoOff::Enabled => {
                                                    screen::UiNotice::DisplayAutoOffOn
                                                }
                                                screen::display::DisplayAutoOff::Disabled => {
                                                    screen::UiNotice::DisplayAutoOffOff
                                                }
                                            };
                                            show_notice(
                                                &mut ui_state,
                                                &mut notice_timer,
                                                notice,
                                                NOTICE_DURATION,
                                            );
                                        }
                                        Err(error) => {
                                            log::error!(
                                                "display auto-off toggle failed: {error:?}"
                                            );
                                        }
                                    }
                                }
                                screen::UiAction::ControlGnss(command) => {
                                    B::Gnss::control(command);
                                }
                                screen::UiAction::Sleep => {
                                    show_notice(
                                        &mut ui_state,
                                        &mut notice_timer,
                                        screen::UiNotice::Sleeping,
                                        NOTICE_DURATION,
                                    );
                                    if let Err(error) = display.schedule_blanking(
                                        MonotonicMillis::new(
                                            now_ms.saturating_add(DISPLAY_SLEEP_DELAY_MS),
                                        ),
                                        DisplayBlankReason::SystemSleep,
                                    ) {
                                        log::error!(
                                            "system-sleep display scheduling failed: {error:?}"
                                        );
                                    }
                                    usb_status.disable();
                                    if let Some(status) = lora_card_status {
                                        status.disable();
                                    }
                                    if let Some(status) = wifi_status.as_ref() {
                                        status.disable();
                                        status.disable_station_uplink();
                                    }
                                    if let Some(status) = espnow_card_status {
                                        status.disable();
                                    }
                                    if let Some(tcp) = tcp_status {
                                        tcp.disable();
                                    }
                                    {
                                        let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                        status.disable();
                                    }
                                    B::Gnss::control(screen::GnssReceiverCommand::Disable);
                                }
                                screen::UiAction::Wake => {
                                    if let Err(error) = display
                                        .request_visible(MonotonicMillis::new(now_ms), display_now)
                                    {
                                        log::error!("display wake failed: {error:?}");
                                    }
                                    show_notice(
                                        &mut ui_state,
                                        &mut notice_timer,
                                        screen::UiNotice::Awake,
                                        NOTICE_DURATION,
                                    );
                                    usb_status.enable();
                                    if let Some(status) = lora_card_status {
                                        status.enable();
                                    }
                                    if let Some(status) = wifi_status.as_ref() {
                                        status.enable_station_uplink();
                                        status.enable();
                                    }
                                    if let Some(status) = espnow_card_status {
                                        status.enable();
                                    }
                                    if let Some(tcp) = tcp_status {
                                        tcp.enable();
                                    }
                                    {
                                        let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                        status.enable();
                                    }
                                    if ui_state.gnss_visible() {
                                        B::Gnss::control(screen::GnssReceiverCommand::Enable);
                                    }
                                }
                                screen::UiAction::Announce => {
                                    boot_stage(BootPhase::AnnounceBegin);
                                    show_notice(
                                        &mut ui_state,
                                        &mut notice_timer,
                                        screen::UiNotice::Announcing,
                                        NOTICE_DURATION,
                                    );
                                    let node_queued =
                                        handle.issue(PrnsCommand::AnnounceNow(AnnounceNow {
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
                                            let notice = if enabled {
                                                screen::UiNotice::TurningOff
                                            } else {
                                                screen::UiNotice::TurningOn
                                            };
                                            show_notice(
                                                &mut ui_state,
                                                &mut notice_timer,
                                                notice,
                                                NOTICE_DURATION,
                                            );
                                        };
                                        if card.id() == usb_status.id() {
                                            show_toggle_notice(usb_status.is_enabled());
                                            usb_status.toggle_enabled();
                                            handled = true;
                                        }
                                        if !handled && Some(card.id()) == lora_card_id {
                                            if let Some(status) = lora_card_status {
                                                show_toggle_notice(status.is_enabled());
                                                status.toggle_enabled();
                                                handled = true;
                                            }
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
                                            if let (Some(tcp), Some(tcp_id)) = (tcp_status, tcp_id)
                                            {
                                                if card.id() == tcp_id {
                                                    show_toggle_notice(tcp.is_enabled());
                                                    tcp.toggle_enabled();
                                                    {
                                                        handled = true;
                                                    }
                                                }
                                            }
                                        }
                                        if !handled && card.id() == BLE_SUPERVISOR_ID {
                                            let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                            show_toggle_notice(status.is_enabled());
                                            status.toggle_enabled();
                                        }
                                    }
                                }
                                screen::UiAction::ToggleStationUplink => {
                                    if let Some(status) = wifi_status.as_ref() {
                                        let notice = if status.is_station_uplink_enabled() {
                                            screen::UiNotice::DisconnectingAp
                                        } else {
                                            screen::UiNotice::ReconnectingAp
                                        };
                                        show_notice(
                                            &mut ui_state,
                                            &mut notice_timer,
                                            notice,
                                            NOTICE_DURATION,
                                        );
                                        status.toggle_station_uplink();
                                    }
                                }
                                screen::UiAction::OpenLoRaEditor => {
                                    #[cfg(feature = "lora")]
                                    ui_state.open_lora_editor(working_lora_profile);
                                }
                                #[cfg(not(feature = "lora"))]
                                screen::UiAction::SetLoRaProfile(_)
                                | screen::UiAction::ResetLoRaProfile => {}
                                #[cfg(feature = "lora")]
                                screen::UiAction::SetLoRaProfile(profile) => {
                                    let result = screen::apply_and_persist_radio_profile(
                                        async {
                                            LORA_CONTROL.apply(profile).await
                                                == LoRaApplyOutcome::Applied
                                        },
                                        || async {
                                            match lora_profile_store.save(profile).await {
                                                Ok(()) => true,
                                                Err(error) => {
                                                    log::error!(
                                                        "LoRa profile save failed: {error:?}"
                                                    );
                                                    false
                                                }
                                            }
                                        },
                                    )
                                    .await;
                                    if result.applied() {
                                        working_lora_profile = profile;
                                    }
                                    let notice = result.notice();
                                    show_notice(
                                        &mut ui_state,
                                        &mut notice_timer,
                                        notice,
                                        NOTICE_DURATION,
                                    );
                                }
                                #[cfg(feature = "lora")]
                                screen::UiAction::ResetLoRaProfile => {
                                    let result = screen::apply_and_persist_radio_profile(
                                        async {
                                            LORA_CONTROL
                                                .apply(boot_lora_profile(B::MAX_TX_POWER_DBM))
                                                .await
                                                == LoRaApplyOutcome::Applied
                                        },
                                        || async {
                                            match lora_profile_store.reset().await {
                                                Ok(()) => true,
                                                Err(error) => {
                                                    log::error!(
                                                        "LoRa profile reset failed: {error:?}"
                                                    );
                                                    false
                                                }
                                            }
                                        },
                                    )
                                    .await;
                                    if result.applied() {
                                        working_lora_profile =
                                            boot_lora_profile(B::MAX_TX_POWER_DBM);
                                    }
                                    let notice = result.notice();
                                    show_notice(
                                        &mut ui_state,
                                        &mut notice_timer,
                                        notice,
                                        NOTICE_DURATION,
                                    );
                                }
                                screen::UiAction::SwapRadioMode => {
                                    let next = match radio_mode {
                                        RadioMode::Ble => RadioMode::AccessPoint,
                                        RadioMode::AccessPoint => RadioMode::Ble,
                                    };
                                    request_radio_mode(next);
                                }
                                screen::UiAction::OpenDocs => {}
                                screen::UiAction::CopySharedInstanceConfig => {}
                                screen::UiAction::None => {}
                            }
                        }
                        if index + 1 == BUTTON_EVENT_CAPACITY {
                            break;
                        }
                        next_event = BUTTON_EVENTS.try_receive().ok();
                    }
                }
            }
        }
    };

    spawner.spawn(watchdog_task(rtc.rwdt).expect("watchdog task fits"));

    if B::Gnss::AVAILABILITY == screen::GnssAvailability::Available {
        let run = crate::storage::allocate_psram(gnss.drive());
        let run: core::pin::Pin<&'static mut dyn core::future::Future<Output = ()>> =
            // SAFETY: `allocate_psram` leaks this allocation, so it cannot move or be freed.
            unsafe { core::pin::Pin::new_unchecked(run) };
        spawner.spawn(gnss_task(run).expect("GNSS task fits"));
    }

    #[cfg(feature = "lora")]
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
                    .with_task_priority(BLE_CONTROLLER_TASK_PRIORITY)
                    .with_task_stack_size(4096)
                    .with_max_activities(BLE_CONTROLLER_ACTIVITY_CAPACITY),
            )
            .expect("ble connector");
            boot_stage(BootPhase::BluetoothReady);
            if let Some((identity, fleet)) = ble {
                let run = crate::storage::allocate_psram(crate::bluetooth_auto::run(
                    ble_connector,
                    mac_octets,
                    identity,
                    fleet,
                    &BLE_SHARED,
                    spawner,
                ));
                let run: core::pin::Pin<&'static mut dyn core::future::Future<Output = ()>> =
                    // SAFETY: `allocate_psram` leaks this allocation, so it cannot move or be freed.
                    unsafe { core::pin::Pin::new_unchecked(run) };
                spawner.spawn(ble_task(run).expect("Bluetooth task fits"));
            }
        }
        RadioMode::AccessPoint => {
            let _ = (bluetooth, ble);
        }
    }
    // The display loop is an independent forever-task. Polling it inline makes the compiler fold
    // its large UI state machine into this boot future's native poll frame, leaving essentially no
    // guarded core-0 stack for callees. Keep the suspended state in PSRAM and hand Embassy only a
    // fat pointer, as we do for the Wi-Fi, BLE, and manifold state machines.
    let render = crate::storage::allocate_psram(render);
    let render: core::pin::Pin<&'static mut dyn core::future::Future<Output = ()>> =
        // SAFETY: `allocate_psram` leaks this allocation, so it cannot move or be freed.
        unsafe { core::pin::Pin::new_unchecked(render) };
    spawner.spawn(display_task(render).expect("display task fits"));
}

#[cfg(feature = "lora")]
#[embassy_executor::task]
async fn lora_task(interface: S3LoraInterface, seam: S3LoraSeam) {
    interface.run(seam).await
}

#[embassy_executor::task]
async fn espnow_task(interface: S3EspNowInterface, seam: S3EspNowSeam) {
    interface.run(seam).await
}

#[embassy_executor::task]
async fn tcp_task(interface: TcpClient<'static>, seam: S3TcpSeam) {
    allocator_api2::boxed::Box::pin_in(interface.run(seam), crate::storage::PsramAlloc).await
}

#[embassy_executor::task]
async fn wifi_task(run: core::pin::Pin<&'static mut dyn core::future::Future<Output = ()>>) {
    run.await
}

#[embassy_executor::task]
async fn gnss_task(run: core::pin::Pin<&'static mut dyn core::future::Future<Output = ()>>) {
    run.await
}

#[embassy_executor::task]
async fn manifold_task(run: core::pin::Pin<&'static mut dyn core::future::Future<Output = ()>>) {
    run.await
}

async fn manifold_run(
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

#[embassy_executor::task]
async fn ble_task(run: core::pin::Pin<&'static mut dyn core::future::Future<Output = ()>>) {
    run.await
}

#[embassy_executor::task]
async fn display_task(run: core::pin::Pin<&'static mut dyn core::future::Future<Output = ()>>) {
    run.await
}
