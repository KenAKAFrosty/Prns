use super::*;
use personal_hopspot_core::display::{
    DisplayDuration, DisplayVisibility, MonotonicMillis, PresentationUrgency,
};
#[cfg(feature = "remote-control-pairing")]
use personal_rns::engine::{OpenRemoteControlPairing, RemoteControlTargetPairingApproval};
use personal_rns::remote_control::{
    RemoteControlInitialControllerGrants, RemoteControlSelfAnnouncement, RemoteControlService,
};
#[cfg(feature = "remote-control-pairing")]
use personal_rns::remote_control::{
    RemoteControlPairingAttemptTimeout, RemoteControlPairingExpiresAfter,
    RemoteControlPairingPublicAppDataBytes,
};

#[cfg(feature = "remote-control-pairing")]
const REMOTE_CONTROL_PAIRING_WINDOW_MILLIS: u64 = 120_000;
#[cfg(feature = "remote-control-pairing")]
const REMOTE_CONTROL_PAIRING_ATTEMPT_MILLIS: u64 = 60_000;

enum RenderLoopEvent {
    Button(screen::InputEvent),
    Tick,
    InterfacesChanged,
    RemoteControlCommand(screen::PendingHopspotCommand),
    #[cfg(feature = "remote-control-pairing")]
    RemoteControlChanged,
}

use crate::memory::EspFirmwareMemory;

fn display_now() -> MonotonicMillis {
    MonotonicMillis::new(embassy_time::Instant::now().as_millis())
}

fn issue_node_page_announce(
    handle: &Handle,
    destination: personal_rns::wire::DestinationHash,
) -> bool {
    handle
        .issue(PrnsCommand::AnnounceNow(AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        }))
        .is_some()
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
    let memory = EspFirmwareMemory::new(B::MEMORY_PROFILE);
    let BoardFace {
        display: board_display,
        battery,
        button,
    } = hardware.face;
    let S3RuntimeBootstrap {
        identities:
            crate::identity::S3IdentityBootstraps {
                node: node_bootstrap,
                remote_control: remote_control_bootstrap,
                ble: ble_bootstrap,
                destination_hashes,
            },
        entropy: mut boot_entropy,
    } = hardware.runtime_bootstrap;
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
    let (mut wifi_config, mut wifi_config_source) = hopspot_wifi_config();

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
        memory.flash_capacity(),
    )));
    let shared_flash = SharedNorFlash::new(flash, memory.flash_capacity());
    let remote_control_bootstrap =
        remote_control_bootstrap.expect("RemoteControl identity bootstrap failed");
    let (remote_control_identity_secrets, _remote_control_identity_origins) =
        remote_control_bootstrap.into_parts();
    let wifi_configuration_key = remote_control_identity_secrets
        .target_sealing_key(screen::WIFI_CONFIGURATION_SEALING_DOMAIN);
    let mut wifi_configuration_store =
        screen::WifiConfigurationStore::new(shared_flash, memory.wifi_configuration_pages());
    let mut runtime_wifi_station = None;
    match wifi_configuration_store
        .load(&wifi_configuration_key, &mut boot_entropy)
        .await
    {
        Ok(loaded) => {
            if let Some(station) = loaded.active {
                wifi_config.ssid = station.ssid().to_string();
                wifi_config_source = HopspotWifiConfigSource::RuntimeSealed;
                runtime_wifi_station = Some(station);
            }
            if loaded.recovered_unconfirmed_transaction {
                log::warn!("wifi-config: restored the last confirmed revision after reboot");
            }
        }
        Err(error) => {
            log::error!("wifi-config: sealed configuration load failed: {error:?}");
        }
    }
    // Keep sealed credentials in their move-only, zeroizing representation. Immutable provisioning
    // is the legacy fallback and crosses into that representation only once at task ownership.
    let initial_wifi_station = runtime_wifi_station.or_else(|| {
        if wifi_config.has_station() {
            personal_rns::remote_control::RemoteControlWifiStation::parse(
                &wifi_config.ssid,
                &wifi_config.password,
            )
            .ok()
        } else {
            None
        }
    });
    let station_configured = initial_wifi_station.is_some();
    let radio_mode = boot_radio_mode(station_configured);
    log::info!(
        "wifi-config source={wifi_config_source:?} station={} ssid_len={} tcp={}",
        station_configured,
        wifi_config.ssid.len(),
        wifi_config.tcp_client.is_some()
    );
    #[cfg(feature = "lora")]
    let mut subg_configuration_store =
        screen::SubGConfigurationStore::new(shared_flash, memory.radio_profile_pages());
    #[cfg(feature = "lora")]
    let loaded_subg_configuration = match subg_configuration_store.load().await {
        Ok(loaded) => loaded,
        Err(error) => {
            log::error!("SubG configuration restore failed: {error:?}");
            screen::LoadedSubGConfiguration {
                state: SubGConfigurationState::Unconfigured,
                notice: Some(screen::SubGConfigurationLoadNotice::Reset),
            }
        }
    };
    #[cfg(feature = "lora")]
    let subg_configuration = loaded_subg_configuration.state;
    #[cfg(feature = "lora")]
    let subg_startup_notice = loaded_subg_configuration.notice.map(|notice| match notice {
        screen::SubGConfigurationLoadNotice::Migrated => screen::UiNotice::SubGMigrated,
        screen::SubGConfigurationLoadNotice::Recovered => screen::UiNotice::SubGRecovered,
        screen::SubGConfigurationLoadNotice::Reset => screen::UiNotice::SubGReset,
    });
    #[cfg(not(feature = "lora"))]
    let subg_startup_notice: Option<screen::UiNotice> = None;
    #[cfg(feature = "lora")]
    let lora_status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new_accounted(
            LoRaInterface::<LoraRadio>::unconfigured_interface_id(),
            ConnectionState::Initializing,
        )
    );
    #[cfg(feature = "lora")]
    let lora_spectrum: &'static LoRaSpectrumStatus =
        mk_static!(LoRaSpectrumStatus, LoRaSpectrumStatus::new());
    // The Option shims mirror ESP-NOW's: boards without an SX1262 keep every downstream card,
    // toggle, and sleep path compiling against `None` instead of forking the render loop.
    #[cfg(feature = "lora")]
    let lora_card_status: Option<&'static EmbassyInterfaceStatus> = Some(lora_status);
    #[cfg(not(feature = "lora"))]
    let lora_card_status: Option<&'static EmbassyInterfaceStatus> = None;
    // Reclaim the private R8 probe allocation before placing the live LoRa queue in PSRAM.
    // This is a no-op on boards whose PSRAM belongs to the global heap.
    #[cfg(feature = "lora")]
    crate::storage::reinit_private_psram_heap();
    #[cfg(feature = "lora")]
    let lora_tx_queue = crate::storage::allocate_lora_tx_queue();
    #[cfg(feature = "lora")]
    let (mut lora_controller, lora_control) = LORA_CONTROL.init(LoRaControl::new()).split();
    #[cfg(feature = "lora")]
    let lora = match LoRaInterface::new(LoRaInterfaceInput {
        radio: lora_radio,
        configuration: subg_configuration,
        airtime_policy: AirtimePolicy::Regional,
        tx_queue: lora_tx_queue,
        control: lora_control,
        status: lora_status,
        spectrum: lora_spectrum,
        lifecycle: LIFECYCLE.dyn_sender(),
    }) {
        Ok(lora) => lora,
        Err(_) => panic!("the built-in LoRa profile and regional policy must be valid"),
    };
    #[cfg(feature = "lora")]
    lora_status.set_id(lora.id());

    crate::identity::log_persistence("node", node_bootstrap.persistence());
    crate::identity::log_persistence("Bluetooth", ble_bootstrap.persistence());

    // The Wi-Fi stack carries both the Wi-Fi Auto UDP and the TCP client, so it stands up before the
    // node moves to core 1 — activating the TCP slot is a core-0-only act.
    boot_stage(BootPhase::WifiBegin);
    let (wifi, tcp_stack, esp_now) = build_wifi(
        &spawner,
        wifi_hardware,
        boot_entropy,
        mac_octets,
        initial_wifi_station,
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
    #[cfg(feature = "remote-control-pairing")]
    let stable_target_destination = remote_control_identity_secrets
        .identities()
        .target()
        .endpoint()
        .destination_hash();
    #[cfg(feature = "firmware-update")]
    let ota_destination = remote_control_identity_secrets
        .identities()
        .target()
        .firmware_update_destination();
    #[cfg(feature = "firmware-update")]
    super::firmware_update_listener::set_destination(ota_destination);
    let mut remote_control = RemoteControlService::with_capabilities(
        remote_control_identity_secrets,
        RemoteControlInitialControllerGrants::Nobody,
        RemoteControlSelfAnnouncement::Destination(destination_hashes.node_page),
        remote_control::capabilities::<B>(),
    );
    #[cfg(feature = "firmware-update")]
    {
        remote_control = remote_control.with_firmware_update_destination();
    }
    #[cfg(feature = "remote-control-pairing")]
    let remote_control_pairing_permissions =
        screen::full_remote_control_pairing_permissions(&remote_control.available_requests())
            .expect("RemoteControl supports at least Describe");
    #[cfg(feature = "remote-control-pairing")]
    let remote_control_pairing_public_app_data =
        RemoteControlPairingPublicAppDataBytes::try_from(B::NODE_ANNOUNCE_APP_DATA)
            .expect("the board's product name fits pairing public app data");
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

    let remote_control_handle = REMOTE_CONTROL_COMMANDS.handle();
    let recipe = PrnsNodeRecipe {
        transport_identity: Some(transport_secret),
        remote_control,
        pre_configured_destinations: destinations.into_preconfigured_destinations(),
        app_state: remote_control_handle,
        storage: EngineStorageType::default(),
        request_endpoints: screen::node_pages::NodePageRoutes,
        interfaces: personal_rns::runtime::ManuallyAttached,
        persistence: crate::persistence::s3(shared_flash, &memory),
        on_event: firmware_on_event as for<'a> fn(PrnsEvent<'a>, &RemoteControlHandle),
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
            device_descriptor(
                usb_id,
                personal_rns::interfaces::usb_auto::DEVICE_USB_BITRATE_BPS,
            ),
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
    let entropy = runtime_entropy();
    #[cfg(feature = "remote-control-pairing")]
    set_remote_control_clock(timebase.now());
    let host = EmbassyHost::new_with_timebase(timebase, entropy);

    let core1_stack = mk_static!(CpuStack<CORE1_STACK_BYTES>, CpuStack::new());
    boot_stage(BootPhase::CoreOneStartBegin);
    esp_rtos::start_second_core(cpu_control, software_interrupt, core1_stack, move || {
        let node_slot = crate::storage::allocate_psram_uninit::<S3Node>();
        let (node, persistence) =
            PrnsNode::init_in_place_with_persistence(node_slot, recipe, manifold_wiring, host);
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
    let usb_seam = usb_lane.into_seam(NOTIFY.sender(), entropy);
    spawner.spawn(usb_device_task(usb_rx, usb_tx, usb_seam, usb_status).expect("usb task fits"));

    #[cfg(feature = "lora")]
    let lora_seam = lora_lane.into_seam(NOTIFY.sender(), entropy);

    let espnow = espnow.zip(espnow_lane).map(|(interface, lane)| {
        let seam = lane.into_seam(NOTIFY.sender(), entropy);
        (interface, seam)
    });

    let tcp = tcp_built.zip(tcp_lane).map(|((tcp, _, _), lane)| {
        let seam = lane.into_seam(NOTIFY.sender(), entropy);
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
    #[cfg(feature = "remote-control-pairing")]
    if B::REMOTE_CONTROL_PAIRING {
        spawner.spawn(
            stable_target_announcer_task(handle, node_page_destination, stable_target_destination)
                .expect("stable target announcer task fits"),
        );
        spawner.spawn(pairing_close_task(handle).expect("pairing close task fits"));
    }

    let wifi_status = wifi.as_ref().map(|(interface, _)| interface.status());
    let wifi_id = wifi_status.as_ref().map(|status| {
        use personal_rns::interfaces::InterfaceStatus;
        status.id()
    });
    if let Some(groups) = personal_rns::runtime::restored_discovery_groups(BLE_SUPERVISOR_ID).await
    {
        let _ = personal_rns::bluetooth_auto::BluetoothAutoStatus::new(&BLE_SHARED)
            .restore_discovery_groups_before_start(groups);
    }
    if let Some(status) = wifi_status.as_ref() {
        if let Some(groups) = personal_rns::runtime::restored_discovery_groups(status.id()).await {
            let _ = status.restore_discovery_groups_before_start(groups);
        }
    }
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
            discovery_groups: screen::DiscoveryGroupEditorAvailability::Available,
            #[cfg(feature = "remote-control-pairing")]
            remote_control_pairing: if B::REMOTE_CONTROL_PAIRING {
                screen::RemoteControlPairingAvailability::Available
            } else {
                screen::RemoteControlPairingAvailability::Unavailable
            },
        });
        let startup_notice = identity_startup_notice.or(subg_startup_notice);
        let mut pending_startup_notice = identity_startup_notice
            .is_some()
            .then_some(subg_startup_notice)
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
        let mut working_subg_configuration = subg_configuration;
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
        let mut wifi_confirmation = None;
        let mut scheduled_remote_control_effect = None;
        let mut system = remote_control::SystemIntent::from_status(
            usb_status,
            lora_card_status,
            wifi_status.as_ref(),
            espnow_card_status,
            tcp_status,
            ui_state.gnss_visible(),
        );
        macro_rules! execute_hopspot_command {
            ($snapshots:expr, $command:expr) => {
                remote_control::execute::<B>(
                    remote_control::S3RemoteControlContext {
                        snapshots: &$snapshots,
                        usb_status,
                        lora_status: lora_card_status,
                        wifi_status: wifi_status.as_ref(),
                        espnow_status: espnow_card_status,
                        tcp_status,
                        wifi_config: &mut wifi_config,
                        power: battery_state,
                        display: &mut display,
                        radio_mode,
                        system: &mut system,
                        scheduled_effect: &mut scheduled_remote_control_effect,
                        wifi_store: &mut wifi_configuration_store,
                        wifi_key: &wifi_configuration_key,
                        wifi_confirmation: &mut wifi_confirmation,
                        #[cfg(feature = "lora")]
                        lora_controller: &mut lora_controller,
                        #[cfg(feature = "lora")]
                        subg_store: &mut subg_configuration_store,
                        #[cfg(feature = "lora")]
                        subg_configuration: &mut working_subg_configuration,
                    },
                    $command,
                )
                .await
            };
        }
        loop {
            remote_control::apply_scheduled::<B>(
                &mut scheduled_remote_control_effect,
                usb_status,
                lora_card_status,
                wifi_status.as_ref(),
                espnow_card_status,
                tcp_status,
                &mut display,
                &mut system,
                &mut wifi_config,
                &mut wifi_confirmation,
            );
            remote_control::rollback_expired(
                &mut wifi_configuration_store,
                &wifi_configuration_key,
                &mut wifi_confirmation,
                &mut wifi_config,
            )
            .await;
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
            #[cfg(feature = "lora")]
            let subg_card_configuration = Some(working_subg_configuration);
            #[cfg(not(feature = "lora"))]
            let subg_card_configuration = None;
            #[cfg(feature = "remote-control-pairing")]
            publish_transmit_egress_ready(
                B::REMOTE_CONTROL_PAIRING
                    && snapshots
                        .iter()
                        .any(|snapshot| snapshot.connection.is_online()),
            );
            #[cfg(feature = "remote-control-pairing")]
            ui_state.sync_remote_control(current_remote_control_state(), remote_control_now());
            let tcp_card_config = wifi_config.tcp_client.as_ref();
            let mut cards = build_cards(
                &snapshots,
                subg_card_configuration,
                usb_status.id(),
                wifi_id,
                tcp_id,
                tcp_card_config,
                wifi_status.as_ref(),
                &wifi_config,
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

            #[cfg(feature = "remote-control-pairing")]
            let loop_event = match select5(
                BUTTON_EVENTS.receive(),
                render_tick.next(),
                INTERFACE_STORE.changed(),
                REMOTE_CONTROL_COMMANDS.receive(),
                REMOTE_CONTROL_UI_WAKE.wait(),
            )
            .await
            {
                Either5::First(event) => RenderLoopEvent::Button(event),
                Either5::Second(()) => RenderLoopEvent::Tick,
                Either5::Third(()) => RenderLoopEvent::InterfacesChanged,
                Either5::Fourth(pending) => RenderLoopEvent::RemoteControlCommand(pending),
                Either5::Fifth(()) => RenderLoopEvent::RemoteControlChanged,
            };
            #[cfg(not(feature = "remote-control-pairing"))]
            let loop_event = match select4(
                BUTTON_EVENTS.receive(),
                render_tick.next(),
                INTERFACE_STORE.changed(),
                REMOTE_CONTROL_COMMANDS.receive(),
            )
            .await
            {
                Either4::First(event) => RenderLoopEvent::Button(event),
                Either4::Second(()) => RenderLoopEvent::Tick,
                Either4::Third(()) => RenderLoopEvent::InterfacesChanged,
                Either4::Fourth(pending) => RenderLoopEvent::RemoteControlCommand(pending),
            };

            match loop_event {
                RenderLoopEvent::RemoteControlCommand(pending) => {
                    let (token, command) = pending.into_parts();
                    let result = execute_hopspot_command!(snapshots, command);
                    REMOTE_CONTROL_COMMANDS.complete(token, result);
                    presentation_urgency = PresentationUrgency::Immediate;
                }
                #[cfg(feature = "remote-control-pairing")]
                RenderLoopEvent::RemoteControlChanged => {
                    presentation_urgency = PresentationUrgency::Immediate;
                }
                RenderLoopEvent::InterfacesChanged => {
                    settle_after_draw = true;
                    presentation_urgency = PresentationUrgency::Telemetry;
                }
                RenderLoopEvent::Tick => {
                    ticks_to_battery_sample = ticks_to_battery_sample.saturating_sub(1);
                    ticks_to_battery_display = ticks_to_battery_display.saturating_sub(1);
                    presentation_urgency = PresentationUrgency::Telemetry;
                }
                RenderLoopEvent::Button(first_event) => {
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
                                    if let Err(error) = execute_hopspot_command!(snapshots,
                                        personal_rns::runtime::RemoteControlHostCommand::SetDisplayVisibility {
                                            visibility: personal_rns::remote_control::RemoteControlDisplayVisibility::Hidden,
                                        }
                                    ) {
                                        log::error!("display-off apply failed: {error:?}");
                                    }
                                }
                                screen::UiAction::ToggleDisplayAutoOff => {
                                    let desired = match display.auto_off() {
                                        Ok(screen::display::DisplayAutoOff::Enabled) => {
                                            personal_rns::remote_control::RemoteControlDisplayAutoOff::Disabled
                                        }
                                        Ok(screen::display::DisplayAutoOff::Disabled) => {
                                            personal_rns::remote_control::RemoteControlDisplayAutoOff::Enabled
                                        }
                                        Err(error) => {
                                            log::error!("display auto-off read failed: {error:?}");
                                            continue;
                                        }
                                    };
                                    match execute_hopspot_command!(snapshots,
                                        personal_rns::runtime::RemoteControlHostCommand::SetDisplayAutoOff {
                                            auto_off: desired,
                                        }
                                    ) {
                                        Ok(_) => {
                                            let notice = if desired
                                                == personal_rns::remote_control::RemoteControlDisplayAutoOff::Enabled
                                            {
                                                screen::UiNotice::DisplayAutoOffOn
                                            } else {
                                                screen::UiNotice::DisplayAutoOffOff
                                            };
                                            show_notice(
                                                &mut ui_state,
                                                &mut notice_timer,
                                                notice,
                                                NOTICE_DURATION,
                                            );
                                        }
                                        Err(error) => log::error!(
                                            "display auto-off apply failed: {error:?}"
                                        ),
                                    }
                                }
                                screen::UiAction::ControlGnss(command) => {
                                    let power = match command {
                                        screen::GnssReceiverCommand::Enable => {
                                            personal_rns::remote_control::RemoteControlGnssPower::On
                                        }
                                        screen::GnssReceiverCommand::Disable => {
                                            personal_rns::remote_control::RemoteControlGnssPower::Off
                                        }
                                    };
                                    if let Err(error) = execute_hopspot_command!(snapshots,
                                        personal_rns::runtime::RemoteControlHostCommand::SetGnssPower { power }
                                    ) {
                                        log::error!("GNSS control failed: {error:?}");
                                    }
                                }
                                screen::UiAction::Sleep => {
                                    show_notice(
                                        &mut ui_state,
                                        &mut notice_timer,
                                        screen::UiNotice::Sleeping,
                                        NOTICE_DURATION,
                                    );
                                    if let Err(error) = execute_hopspot_command!(snapshots,
                                        personal_rns::runtime::RemoteControlHostCommand::SetSystemPower {
                                            power: personal_rns::remote_control::RemoteControlSystemPower::Asleep,
                                        }
                                    ) {
                                        log::error!("system sleep scheduling failed: {error:?}");
                                    }
                                }
                                screen::UiAction::Wake => {
                                    show_notice(
                                        &mut ui_state,
                                        &mut notice_timer,
                                        screen::UiNotice::Awake,
                                        NOTICE_DURATION,
                                    );
                                    if let Err(error) = execute_hopspot_command!(snapshots,
                                        personal_rns::runtime::RemoteControlHostCommand::SetSystemPower {
                                            power: personal_rns::remote_control::RemoteControlSystemPower::Awake,
                                        }
                                    ) {
                                        log::error!("system wake failed: {error:?}");
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
                                    #[cfg(feature = "remote-control-pairing")]
                                    {
                                        if B::REMOTE_CONTROL_PAIRING {
                                            request_manual_announcements();
                                        } else {
                                            let node_queued = issue_node_page_announce(
                                                &handle,
                                                node_page_destination,
                                            );
                                            log::info!(
                                                "announce-ui destination=node queued={node_queued}"
                                            );
                                        }
                                    }
                                    #[cfg(not(feature = "remote-control-pairing"))]
                                    {
                                        let node_queued = issue_node_page_announce(
                                            &handle,
                                            node_page_destination,
                                        );
                                        log::info!(
                                            "announce-ui destination=node queued={node_queued}"
                                        );
                                    }
                                    boot_stage(BootPhase::AnnounceNodeIssueReturned);
                                    #[cfg(feature = "remote-control-pairing")]
                                    if B::REMOTE_CONTROL_PAIRING {
                                        log::info!(
                                            "announce-ui destination=node+remote queued=true"
                                        );
                                    }
                                }
                                #[cfg(feature = "remote-control-pairing")]
                                screen::UiAction::OpenRemoteControlPairing => {
                                    let _ = update_remote_control_state(
                                        screen::RemoteControlTargetPairingState::begin_opening,
                                    );
                                    let open = OpenRemoteControlPairing {
                                        target: AnnounceTarget::AllInterfaces,
                                        expires_after: RemoteControlPairingExpiresAfter::try_from(
                                            personal_rns::units::DurationMillis(
                                                REMOTE_CONTROL_PAIRING_WINDOW_MILLIS,
                                            ),
                                        )
                                        .expect("the pairing window is valid"),
                                        attempt_timeout:
                                            RemoteControlPairingAttemptTimeout::try_from(
                                                personal_rns::units::DurationMillis(
                                                    REMOTE_CONTROL_PAIRING_ATTEMPT_MILLIS,
                                                ),
                                            )
                                            .expect("the pairing attempt timeout is valid"),
                                        permissions: remote_control_pairing_permissions.clone(),
                                        public_app_data:
                                            remote_control_pairing_public_app_data.clone(),
                                    };
                                    match handle.open_remote_control_pairing(open).await {
                                        Ok(opened) => {
                                            log::info!(
                                                "remote-control pairing opened endpoint={:?} expires_at={}",
                                                opened.endpoint.destination_hash(),
                                                opened.expires_at.0
                                            );
                                            let invitation_code = opened.invitation_code.value();
                                            let expires_at = opened.expires_at;
                                            let _ = update_remote_control_state(|state| {
                                                state.opened(invitation_code, expires_at)
                                            });
                                        }
                                        Err(error) => {
                                            log::error!(
                                                "remote-control pairing open failed: {error:?}"
                                            );
                                            let _ = update_remote_control_state(|state| {
                                                state.operation_failed(
                                                    None,
                                                    screen::RemoteControlTargetPairingFailure::Open,
                                                )
                                            });
                                        }
                                    }
                                }
                                #[cfg(feature = "remote-control-pairing")]
                                screen::UiAction::CloseRemoteControlPairing => {
                                    match handle.close_remote_control_pairing().await {
                                        Ok(outcome) => {
                                            log::info!(
                                                "remote-control pairing close settled: {outcome:?}"
                                            );
                                            let _ = update_remote_control_state(
                                                screen::RemoteControlTargetPairingState::cancelled,
                                            );
                                        }
                                        Err(error) => {
                                            log::error!(
                                                "remote-control pairing close failed: {error:?}"
                                            );
                                            let _ = update_remote_control_state(|state| {
                                                state.operation_failed(
                                                    state.attempt_id(),
                                                    screen::RemoteControlTargetPairingFailure::Close,
                                                )
                                            });
                                            ui_state.remote_control_pairing_close_failed();
                                        }
                                    }
                                }
                                #[cfg(feature = "remote-control-pairing")]
                                screen::UiAction::ApproveRemoteControlTargetPairing(attempt_id) => {
                                    match handle
                                        .approve_remote_control_target_pairing(
                                            personal_rns::engine::ApproveRemoteControlTargetPairing {
                                                attempt_id,
                                            },
                                        )
                                        .await
                                    {
                                        Ok(RemoteControlTargetPairingApproval::AwaitingControllerCommit {
                                            attempt_id,
                                        }) => {
                                            let _ = update_remote_control_state(|state| {
                                                state.awaiting_controller_commit(attempt_id)
                                            });
                                        }
                                        Ok(RemoteControlTargetPairingApproval::AuthorizationOwed {
                                            attempt_id,
                                            ..
                                        }) => {
                                            let _ = update_remote_control_state(|state| {
                                                state.authorizing(attempt_id)
                                            });
                                        }
                                        Err(error) => {
                                            log::error!(
                                                "remote-control target approval failed: {error:?}"
                                            );
                                            let _ = update_remote_control_state(|state| {
                                                state.operation_failed(
                                                    Some(attempt_id),
                                                    screen::RemoteControlTargetPairingFailure::Approval,
                                                )
                                            });
                                        }
                                    }
                                }
                                #[cfg(feature = "remote-control-pairing")]
                                screen::UiAction::RejectRemoteControlTargetPairing(attempt_id) => {
                                    match handle
                                        .reject_remote_control_target_pairing(
                                            personal_rns::engine::RejectRemoteControlTargetPairing {
                                                attempt_id,
                                            },
                                        )
                                        .await
                                    {
                                        Ok(rejection) => {
                                            log::info!(
                                                "remote-control target pairing rejected: {rejection:?}"
                                            );
                                            let _ = update_remote_control_state(|state| {
                                                state.rejected(attempt_id)
                                            });
                                        }
                                        Err(error) => {
                                            log::error!(
                                                "remote-control target rejection failed: {error:?}"
                                            );
                                            let _ = update_remote_control_state(|state| {
                                                state.operation_failed(
                                                    Some(attempt_id),
                                                    screen::RemoteControlTargetPairingFailure::Rejection,
                                                )
                                            });
                                        }
                                    }
                                }
                                screen::UiAction::ToggleSelectedInterface => {
                                    if let Some(card) = ui_state.selected_card(content.cards) {
                                        let enabled = card.connection() != ConnectionState::Disabled;
                                        let power = if enabled {
                                            personal_rns::remote_control::RemoteControlInterfacePower::Off
                                        } else {
                                            personal_rns::remote_control::RemoteControlInterfacePower::On
                                        };
                                        show_notice(
                                            &mut ui_state,
                                            &mut notice_timer,
                                            if enabled {
                                                screen::UiNotice::TurningOff
                                            } else {
                                                screen::UiNotice::TurningOn
                                            },
                                            NOTICE_DURATION,
                                        );
                                        if let Err(error) = execute_hopspot_command!(snapshots,
                                            personal_rns::runtime::RemoteControlHostCommand::SetInterfacePower {
                                                id: card.id(),
                                                power,
                                            }
                                        ) {
                                            log::error!("interface power apply failed: {error:?}");
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
                                        let uplink = if status.is_station_uplink_enabled() {
                                            personal_rns::remote_control::RemoteControlStationUplink::Disabled
                                        } else {
                                            personal_rns::remote_control::RemoteControlStationUplink::Enabled
                                        };
                                        if let Err(error) = execute_hopspot_command!(snapshots,
                                            personal_rns::runtime::RemoteControlHostCommand::SetStationUplink {
                                                id: status.id(),
                                                uplink,
                                            }
                                        ) {
                                            log::error!("station uplink apply failed: {error:?}");
                                        }
                                    }
                                }
                                screen::UiAction::OpenDiscoveryGroupsEditor(id) => {
                                    let groups = if id == BLE_SUPERVISOR_ID {
                                        Some(
                                            personal_rns::bluetooth_auto::BluetoothAutoStatus::new(
                                                &BLE_SHARED,
                                            )
                                            .discovery_groups(),
                                        )
                                    } else {
                                        wifi_status
                                            .as_ref()
                                            .filter(|status| status.id() == id)
                                            .map(|status| status.discovery_groups())
                                    };
                                    if let Some(groups) = groups {
                                        ui_state.open_discovery_groups_editor(id, &groups);
                                    }
                                }
                                screen::UiAction::ReplaceDiscoveryGroups => {
                                    let Some(replacement) =
                                        ui_state.take_discovery_group_replacement()
                                    else {
                                        continue;
                                    };
                                    let id = replacement.interface_id();
                                    let groups = replacement.into_groups();
                                    let result = execute_hopspot_command!(snapshots,
                                        personal_rns::runtime::RemoteControlHostCommand::ReplaceInterfaceDiscoveryGroups {
                                            id,
                                            groups: personal_rns::remote_control::RemoteControlDiscoveryGroups::new(groups),
                                        }
                                    );
                                    let notice = match result {
                                        Ok(personal_rns::runtime::RemoteControlHostResponse::ReplaceInterfaceDiscoveryGroups(
                                            personal_rns::remote_control::RemoteControlDiscoveryGroupsReplaceOutcome::Applied
                                            | personal_rns::remote_control::RemoteControlDiscoveryGroupsReplaceOutcome::Unchanged,
                                        )) => screen::UiNotice::Saved,
                                        Err(personal_rns::runtime::RemoteControlHostCommandError::Busy) => {
                                            screen::UiNotice::GroupsBusy
                                        }
                                        Err(personal_rns::runtime::RemoteControlHostCommandError::PersistenceFailed) => {
                                            screen::UiNotice::GroupsNotSaved
                                        }
                                        Err(personal_rns::runtime::RemoteControlHostCommandError::RollbackFailed) => {
                                            screen::UiNotice::GroupsRollbackFailed
                                        }
                                        _ => screen::UiNotice::GroupsApplyFailed,
                                    };
                                    show_notice(
                                        &mut ui_state,
                                        &mut notice_timer,
                                        notice,
                                        NOTICE_DURATION,
                                    );
                                }
                                screen::UiAction::OpenSubGEditor => {
                                    #[cfg(feature = "lora")]
                                    ui_state.open_subg_editor(working_subg_configuration);
                                }
                                #[cfg(not(feature = "lora"))]
                                screen::UiAction::SetSubGConfiguration(_)
                                | screen::UiAction::ClearSubGConfiguration => {}
                                #[cfg(feature = "lora")]
                                action @ (screen::UiAction::SetSubGConfiguration(_)
                                | screen::UiAction::ClearSubGConfiguration) => {
                                    let requested = if let screen::UiAction::SetSubGConfiguration(
                                        configuration,
                                    ) = action
                                    {
                                        SubGConfigurationState::Configured(configuration)
                                    } else {
                                        SubGConfigurationState::Unconfigured
                                    };
                                    let result = match remote_control::apply_subg_configuration(
                                        &mut lora_controller,
                                        &mut subg_configuration_store,
                                        &mut working_subg_configuration,
                                        requested,
                                    )
                                    .await
                                    {
                                        Ok(()) => screen::SubGConfigurationChangeResult::Saved,
                                        Err(personal_rns::runtime::RemoteControlHostCommandError::ApplyFailed) => {
                                            screen::SubGConfigurationChangeResult::ApplyFailed
                                        }
                                        Err(personal_rns::runtime::RemoteControlHostCommandError::RollbackFailed) => {
                                            screen::SubGConfigurationChangeResult::RollbackFailed
                                        }
                                        Err(_) => {
                                            screen::SubGConfigurationChangeResult::PersistenceFailed
                                        }
                                    };
                                    let notice = result.notice();
                                    show_notice(
                                        &mut ui_state,
                                        &mut notice_timer,
                                        notice,
                                        NOTICE_DURATION,
                                    );
                                }
                                screen::UiAction::SwapRadioMode => {
                                    let mode = match radio_mode {
                                        RadioMode::Ble => personal_rns::remote_control::RemoteControlEspRadioMode::AccessPoint,
                                        RadioMode::AccessPoint => personal_rns::remote_control::RemoteControlEspRadioMode::Bluetooth,
                                    };
                                    if let Err(error) = execute_hopspot_command!(snapshots,
                                        personal_rns::runtime::RemoteControlHostCommand::SetEspRadioMode { mode }
                                    ) {
                                        log::error!("radio-mode scheduling failed: {error:?}");
                                    }
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

    #[cfg(feature = "firmware-update")]
    spawner.spawn(ota_health_task(B::MEMORY_PROFILE).expect("ota health task fits"));
    #[cfg(feature = "firmware-update")]
    spawner.spawn(
        super::firmware_update_listener::firmware_update_listener_task(
            handle.clone(),
            B::MEMORY_PROFILE,
        )
        .expect("firmware update listener fits"),
    );

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
            super::entropy::reseed_after_radio_start();
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

#[cfg(feature = "remote-control-pairing")]
#[embassy_executor::task]
async fn stable_target_announcer_task(
    handle: Handle,
    node_page_destination: personal_rns::wire::DestinationHash,
    stable_target_destination: personal_rns::wire::DestinationHash,
) -> ! {
    loop {
        let now = Instant::now().as_millis();
        if let Some(action) = poll_stable_target_announcement(now) {
            let (destination, kind) = match action {
                screen::StableTargetAnnouncementAction::AutomaticStableTarget { .. } => {
                    (stable_target_destination, "automatic-stable")
                }
                screen::StableTargetAnnouncementAction::ManualNodePage => {
                    (node_page_destination, "manual-node")
                }
                screen::StableTargetAnnouncementAction::ManualStableTarget => {
                    (stable_target_destination, "manual-stable")
                }
            };
            let result = handle
                .announce_now(AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                })
                .await;
            match &result {
                Ok(()) => log::info!(
                    "announce settled kind={kind} destination={destination:?} outcome=sent"
                ),
                Err(error) => log::error!(
                    "announce settled kind={kind} destination={destination:?} outcome=failed error={error:?}"
                ),
            }
            let settled = settle_stable_target_announcement(action, result.is_ok());
            debug_assert!(settled, "the serialized announce action remains in flight");
            continue;
        }

        if let Some(deadline) = next_stable_target_announcement_deadline_millis() {
            let deadline = Instant::try_from_millis(deadline).unwrap_or(Instant::MAX);
            match select(STABLE_TARGET_ANNOUNCER_WAKE.wait(), Timer::at(deadline)).await {
                Either::First(()) | Either::Second(()) => {}
            }
        } else {
            STABLE_TARGET_ANNOUNCER_WAKE.wait().await;
        }
    }
}

#[cfg(feature = "remote-control-pairing")]
#[embassy_executor::task]
async fn pairing_close_task(handle: Handle) -> ! {
    loop {
        PAIRING_CLOSE_WAKE.wait().await;
        if !PAIRING_CLOSE_REQUESTED.swap(false, Ordering::AcqRel) {
            continue;
        }
        match handle.close_remote_control_pairing().await {
            Ok(outcome) => {
                log::warn!("remote-control pairing failed closed: {outcome:?}");
            }
            Err(error) => {
                log::error!("remote-control pairing fail-close command failed: {error:?}");
            }
        }
    }
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
    #[cfg(feature = "remote-control-pairing")]
    let report = node.restore_embedded_persistence(persistence).await;
    #[cfg(not(feature = "remote-control-pairing"))]
    let _ = node.restore_embedded_persistence(persistence).await;
    #[cfg(feature = "remote-control-pairing")]
    {
        set_remote_control_clock(report.logical_start);
        observe_restored_remote_control_grants(
            report.remote_control_controller_grants_restored_count,
        );
    }
    boot_stage(BootPhase::PersistenceRestoreComplete);
    node.run_manifold_with_persistence_and_interface_store(&INTERFACE_STORE, persistence)
        .await
}

/// Confirm the running image once core 1 has proven it is alive. A single-slot board has nothing
/// to confirm. A boot selection that names a slot the node is not executing is repaired onto the
/// running slot here.
#[cfg(feature = "firmware-update")]
#[embassy_executor::task]
async fn ota_health_task(profile: &'static personal_hopspot_memory::MemoryProfile) {
    use crate::firmware_update::{self, SlotHealth};

    loop {
        Timer::after(Duration::from_secs(1)).await;
        if CORE_ONE_HEARTBEAT.load(Ordering::Relaxed) < firmware_update::VALIDATE_HEARTBEATS {
            continue;
        }
        if firmware_update::install_in_progress() {
            continue;
        }
        break;
    }
    let memory = EspFirmwareMemory::new(profile);
    match firmware_update::mark_running_slot_valid(&memory) {
        Ok(SlotHealth::NoOtaSlots) => {
            log::debug!("update: no ota slots on this partition table");
        }
        Ok(SlotHealth::AlreadyValid) => {}
        Ok(SlotHealth::MarkedValid) => log::info!("update: running image marked valid"),
        Ok(SlotHealth::SelectionRepaired { selected, booted }) => {
            log::warn!(
                "update: ota selection named {selected} while running {booted}, repaired to {booted}"
            );
        }
        Err(error) => log::warn!("update: could not mark the running image valid: {error}"),
    }
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
