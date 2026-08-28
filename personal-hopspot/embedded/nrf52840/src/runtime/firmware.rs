use embassy_executor::Spawner;
use embassy_futures::join::{join, join3, join5};
use embassy_futures::select::{select4, Either4};
use embassy_time::{Duration, Instant, Timer};
use embassy_usb::{Builder, Config as UsbConfig};
use static_cell::{ConstStaticCell, StaticCell};

use nrf_softdevice::ble::l2cap;
use nrf_softdevice::Softdevice;

use personal_hopspot_core as hopspot;
use personal_rns::bluetooth_auto::{BluetoothAuto, BluetoothAutoStatus};
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand};
use personal_rns::interfaces::bluetooth_auto::{Endpoint, LinkCapabilities, Nrf52Host, BLE_HW_MTU};
use personal_rns::interfaces::lora::{AirtimePolicy, DEFAULT_915_PROFILE};
use personal_rns::interfaces::usb_auto::{WEBUSB_PRODUCT_ID, WEBUSB_VENDOR_ID};
use personal_rns::interfaces::{ConnectionState, InterfaceStatus};
use personal_rns::lora::{LoRaApplyOutcome, LoRaInterface, LoRaInterfaceInput, LoRaSpectrumStatus};
use personal_rns::manifold::embassy::{EmbassyHost, EmbassyInterfaceStatus};
use personal_rns::manifold::interface_seam::Interface;
use personal_rns::remote_control::{
    RemoteControlInitialAccess, RemoteControlPublicAppData, RemoteControlSelfAnnouncement,
    RemoteControlService,
};
use personal_rns::runtime::{Fleet, PrnsEvent, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe};
use personal_rns::storage::StorageLayout;
use personal_rns::usb_auto::{UsbAutoDevice, UsbAutoDeviceInput};
use personal_rns::usb_auto::{
    WebUsbAutoClass, WebUsbAutoState, WebUsbBootloaderEntry, WEBUSB_AUTO_CONTROL_BUFFER_BYTES,
    WEBUSB_AUTO_MSOS_DESCRIPTOR_BYTES, WEBUSB_AUTO_PACKET_SIZE,
};

use crate::boards::selected as board;
use crate::retained_display::RetainedPresentation;
use board::{
    Board, Controls, DisplayHardware, EarlyHardware, FaceHardware, RuntimeHardware, Storage,
    UsbHardware, ANNOUNCE_APP_DATA, NODE_ANNOUNCE_APP_DATA, USB_INTERFACE_ID, USB_MANUFACTURER,
    USB_PRODUCT, USB_SERIAL_NUMBER,
};

use super::bluetooth_auto::{
    acceptor, scanner, serve_slot, softdevice_config, softdevice_task, usb_vbus_present,
    L2capPacket, NrfBleBackend, Server, BLE_SHARED, BLE_SUPERVISOR_ID, HUB, MEMBERS, OUTBOUND_WAKE,
    POOL,
};
use super::entropy::{initialize_runtime_entropy, runtime_entropy, RUNTIME_ENTROPY_SEED_LEN};
use super::interface_cards::{build_cards, build_snapshots};
use super::node::*;
use hopspot::PresentedNoticeTimer;

const STATS_POLL: Duration = Duration::from_secs(1);
const NOTICE_DURATION: hopspot::display::DisplayDuration =
    match hopspot::display::DisplayDuration::from_millis(900) {
        Ok(duration) => duration,
        Err(_) => panic!("the notice duration is nonzero"),
    };
const STARTUP_NOTICE_DURATION: hopspot::display::DisplayDuration =
    match hopspot::display::DisplayDuration::from_millis(5_000) {
        Ok(duration) => duration,
        Err(_) => panic!("the startup notice duration is nonzero"),
    };
const USB_CONFIG_DESCRIPTOR_BYTES: usize = 64;
const USB_BOS_DESCRIPTOR_BYTES: usize = 64;

fn show_notice(
    state: &mut hopspot::UiState,
    timer: &mut PresentedNoticeTimer,
    notice: hopspot::UiNotice,
    duration: hopspot::display::DisplayDuration,
) {
    state.show_notice(notice);
    timer.stage(notice, duration);
}

fn next_deadline_timer(
    presentation: Option<hopspot::display::MonotonicMillis>,
    notice: Option<hopspot::display::MonotonicMillis>,
) -> Timer {
    let deadline = match (presentation, notice) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    };
    let instant = deadline
        .and_then(|deadline| Instant::try_from_millis(deadline.as_millis()))
        .unwrap_or(Instant::MAX);
    Timer::at(instant)
}

#[embassy_executor::task]
async fn manifold_task(
    node: &'static mut Node,
    persistence: &'static mut super::learned_state::BoardPersistence,
) {
    let _ = node.restore_embedded_persistence(persistence).await;
    node.run_manifold_with_persistence_and_interface_store(&INTERFACE_STORE, persistence)
        .await
}

#[allow(clippy::too_many_lines)]
pub async fn run(spawner: Spawner) -> ! {
    let (
        (node_bootstrap, remote_control_bootstrap, ble_bootstrap, runtime_entropy_seed),
        early_hardware,
    ) = Board::initialize_identities(|nvmc, rng| {
        let mut fill_entropy = |bytes: &mut [u8]| rng.blocking_fill_bytes(bytes);
        let node_bootstrap = board::bootstrap_node_identity(nvmc, &mut fill_entropy);
        let remote_control_bootstrap = board::REMOTE_CONTROL_IDENTITY_FLASH
            .load_or_generate(nvmc, &mut fill_entropy)
            .expect("RemoteControl identity bootstrap failed");
        let ble_bootstrap = board::bootstrap_ble_identity(nvmc, &mut fill_entropy);
        let mut runtime_entropy_seed =
            personal_rns::identity::Zeroizing::new([0u8; RUNTIME_ENTROPY_SEED_LEN]);
        fill_entropy(&mut runtime_entropy_seed[..]);
        (
            node_bootstrap,
            remote_control_bootstrap,
            ble_bootstrap,
            runtime_entropy_seed,
        )
    });
    initialize_runtime_entropy(&runtime_entropy_seed);
    drop(runtime_entropy_seed);
    let identity_startup_notice =
        board::identity_startup_notice(node_bootstrap.persistence(), ble_bootstrap.persistence());
    let node_identity = node_bootstrap.into_identity();
    let (remote_control_identity_secrets, _remote_control_identity_origins) =
        remote_control_bootstrap.into_parts();
    let ble_identity = Some(ble_bootstrap.into_identity());

    let EarlyHardware {
        usb,
        face,
        deferred,
    } = early_hardware;
    let UsbHardware {
        driver: usb_driver,
        vbus,
    } = usb;
    let mut usb_config = UsbConfig::new(WEBUSB_VENDOR_ID, WEBUSB_PRODUCT_ID);
    usb_config.manufacturer = Some(USB_MANUFACTURER);
    usb_config.product = Some(USB_PRODUCT);
    usb_config.serial_number = Some(USB_SERIAL_NUMBER);
    usb_config.max_packet_size_0 = 64;
    static CONFIG_DESC: StaticCell<[u8; USB_CONFIG_DESCRIPTOR_BYTES]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; USB_BOS_DESCRIPTOR_BYTES]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; WEBUSB_AUTO_MSOS_DESCRIPTOR_BYTES]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; WEBUSB_AUTO_CONTROL_BUFFER_BYTES]> = StaticCell::new();
    let mut builder = Builder::new(
        usb_driver,
        usb_config,
        CONFIG_DESC.init([0; USB_CONFIG_DESCRIPTOR_BYTES]),
        BOS_DESC.init([0; USB_BOS_DESCRIPTOR_BYTES]),
        MSOS_DESC.init([0; WEBUSB_AUTO_MSOS_DESCRIPTOR_BYTES]),
        CONTROL_BUF.init([0; WEBUSB_AUTO_CONTROL_BUFFER_BYTES]),
    );
    builder.msos_descriptor(embassy_usb::msos::windows_version::WIN8_1, 0x20);
    static USB_STATE: StaticCell<WebUsbAutoState> = StaticCell::new();
    let class = WebUsbAutoClass::new(
        &mut builder,
        USB_STATE.init(WebUsbAutoState::new(WebUsbBootloaderEntry::Unsupported)),
        WEBUSB_AUTO_PACKET_SIZE,
    );
    let mut usb = builder.build();

    let sd = Softdevice::enable(&softdevice_config());
    static SERVER: StaticCell<Server> = StaticCell::new();
    let server: &'static Server = SERVER.init(Server::new(sd).unwrap());
    static L2CAP: StaticCell<l2cap::L2cap<L2capPacket>> = StaticCell::new();
    let l2cap: &'static l2cap::L2cap<L2capPacket> = L2CAP.init(l2cap::L2cap::init(sd));
    let sd: &'static Softdevice = sd;
    spawner.spawn(softdevice_task(sd, vbus).expect("softdevice task fits"));
    let shared_flash = super::learned_state::take_flash(sd);
    let mut lora_profile_store =
        hopspot::RadioProfileStore::new(shared_flash, board::RADIO_PROFILE_PAGES);
    let loaded_lora_profile = match lora_profile_store.load(DEFAULT_915_PROFILE).await {
        Ok(loaded) => loaded,
        Err(_) => hopspot::LoadedRadioProfile {
            profile: DEFAULT_915_PROFILE,
            follows_default: true,
            notice: Some(hopspot::RadioProfileLoadNotice::Reset),
        },
    };
    let profile_startup_notice = loaded_lora_profile.notice.map(|notice| match notice {
        hopspot::RadioProfileLoadNotice::Recovered => hopspot::UiNotice::ProfileRecovered,
        hopspot::RadioProfileLoadNotice::Reset => hopspot::UiNotice::ProfileReset,
    });
    if let Some(identity) = ble_identity {
        super::bluetooth_auto::set_columba_identity(sd, server, identity);
    }
    if ble_identity.is_some() {
        for idx in 0..POOL {
            spawner.spawn(serve_slot(idx, sd, l2cap, server, &HUB).expect("serve slot fits"));
        }
    }

    let RuntimeHardware {
        radio,
        display,
        controls,
    } = deferred.finish().await;
    let DisplayHardware {
        device: display,
        _rail: _eink_rail,
    } = display;
    let Controls { button, frontlight } = controls;
    let FaceHardware {
        battery: saadc,
        status_led: mut led,
    } = face;

    let transport_secret = node_identity.transport_secret();
    let destination_secret = node_identity.into_destination_secret();
    let destination_hashes = hopspot::HopspotDestinationSet::new(
        destination_secret.clone(),
        ANNOUNCE_APP_DATA,
        NODE_ANNOUNCE_APP_DATA,
    )
    .destination_hashes()
    .expect("the hopspot destination names are valid");
    let node_page_destination = destination_hashes.node_page;
    let remote_control = RemoteControlService::new(
        remote_control_identity_secrets,
        RemoteControlPublicAppData::empty(),
        RemoteControlInitialAccess::Nobody,
        RemoteControlSelfAnnouncement::Destination(node_page_destination),
    );
    let mut manifold_lanes = ManifoldLanes::new();
    let lora_profile = loaded_lora_profile.profile;
    let lora_id = LoRaInterface::<board::Radio>::interface_id(&lora_profile);
    static LORA_STATUS: StaticCell<EmbassyInterfaceStatus> = StaticCell::new();
    let lora_status: &'static EmbassyInterfaceStatus = LORA_STATUS.init(
        EmbassyInterfaceStatus::new_accounted(lora_id, ConnectionState::Initializing),
    );
    static LORA_SPECTRUM: StaticCell<LoRaSpectrumStatus> = StaticCell::new();
    let lora_spectrum: &'static LoRaSpectrumStatus = LORA_SPECTRUM.init(LoRaSpectrumStatus::new());
    static LORA_TX_QUEUE: ConstStaticCell<[u8; LORA_TX_QUEUE_BYTES]> =
        ConstStaticCell::new([0; LORA_TX_QUEUE_BYTES]);
    let lora_tx_queue = LORA_TX_QUEUE.take();
    let lora = match LoRaInterface::new(LoRaInterfaceInput {
        radio,
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

    let (usb_tx, usb_rx) = class.split();
    static USB_STATUS: StaticCell<EmbassyInterfaceStatus> = StaticCell::new();
    let usb_status: &'static EmbassyInterfaceStatus = USB_STATUS.init(
        EmbassyInterfaceStatus::new_accounted(USB_INTERFACE_ID, ConnectionState::Initializing),
    );
    let usb_dev = UsbAutoDevice::new(UsbAutoDeviceInput {
        rx: usb_rx,
        tx: usb_tx,
        status: usb_status,
        host_present: || true,
    });

    let lora_lane = manifold_lanes
        .claim_accounted_interface(&LORA_MANIFOLD_LANE, lora.descriptor(), lora_status)
        .expect("LoRa lane is available");
    let ble_supervisor_lane = ble_identity.as_ref().map(|_| {
        manifold_lanes
            .claim_supervisor(&BLE_MANIFOLD_LANE, BLE_SUPERVISOR_ID, &OUTBOUND_WAKE)
            .expect("Bluetooth supervisor lane is available")
    });
    let usb_lane = manifold_lanes
        .claim_accounted_interface(&USB_MANIFOLD_LANE, usb_dev.descriptor(), usb_status)
        .expect("USB lane is available");

    let handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let manifold_wiring = manifold_lanes.into_manifold_wiring(
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new(runtime_entropy as fn(&mut [u8]));
    static NODE: StaticCell<Node> = StaticCell::new();
    let recipe = PrnsNodeRecipe {
        transport_identity: Some(transport_secret),
        remote_control,
        pre_configured_destinations: hopspot::HopspotDestinationSet::new(
            destination_secret,
            ANNOUNCE_APP_DATA,
            NODE_ANNOUNCE_APP_DATA,
        )
        .into_preconfigured_destinations(),
        app_state: (),
        storage: Storage,
        request_endpoints: hopspot::node_pages::NodePageRoutes,
        interfaces: personal_rns::runtime::ManuallyAttached,
        persistence: super::learned_state::new(shared_flash),
        on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
    };
    let (node, persistence) =
        PrnsNode::init_static_with_persistence(&NODE, recipe, manifold_wiring, host);
    node.set_protocol_policy(personal_hopspot_core::EMBEDDED_HOPSPOT_PROTOCOL_POLICY);
    static PERSISTENCE: StaticCell<super::learned_state::BoardPersistence> = StaticCell::new();
    let persistence = PERSISTENCE.init(persistence);
    spawner.spawn(manifold_task(node, persistence).expect("manifold task fits"));
    let lora_seam = lora_lane.into_seam(NOTIFY.sender(), runtime_entropy);

    let usb_seam = usb_lane.into_seam(NOTIFY.sender(), runtime_entropy);

    let backend = NrfBleBackend::new(&HUB);
    let bluetooth = ble_identity
        .zip(ble_supervisor_lane)
        .map(|(identity, lane)| {
            let supervisor = BluetoothAuto::new(
                backend,
                identity,
                Endpoint::Nrf52(Nrf52Host::Nrf52),
                LinkCapabilities {
                    l2cap: None,
                    link_mtu: BLE_HW_MTU as u16,
                },
                crate::runtime::bluetooth_auto::local_discovery_group_tag(),
                &BLE_SHARED,
            );
            let fleet: Fleet<Mtx, BLE_HW_MTU, NOTIFY_CAP, LIFECYCLE_CAP> =
                lane.into_fleet(NOTIFY.sender(), LIFECYCLE.sender());
            (supervisor, fleet)
        });

    let usb_fut = usb.run();

    let heartbeat = async {
        loop {
            led.set_low();
            Timer::after(super::heartbeat::NORMAL.illuminated()).await;
            led.set_high();
            Timer::after(super::heartbeat::NORMAL.dark()).await;
        }
    };

    let ui_handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let render = async move {
        let mut saadc = saadc;
        let mut display = display.into_runtime(board::retained_policy());
        let mut ui_state = hopspot::UiState::new(hopspot::UiConfiguration {
            storage_limits: <Storage as StorageLayout>::LIMITS,
            user_blanking: display.user_blanking(),
            access_point: hopspot::AccessPointState::Unsupported,
            shared_instance_config_export: hopspot::SharedInstanceConfigExport::Unavailable,
            gnss: hopspot::GnssAvailability::Unavailable,
        });
        let startup_notice = identity_startup_notice.or(profile_startup_notice);
        let mut pending_startup_notice = identity_startup_notice
            .is_some()
            .then_some(profile_startup_notice)
            .flatten();
        let mut notice_timer = PresentedNoticeTimer::new();
        if let Some(notice) = startup_notice {
            show_notice(
                &mut ui_state,
                &mut notice_timer,
                notice,
                STARTUP_NOTICE_DURATION,
            );
        }
        let mut working_lora_profile = lora_profile;
        let mut refresh_urgency = hopspot::display::PresentationUrgency::Immediate;
        let mut activity = hopspot::CardActivityTracker::<{ MEMBERS + 4 }>::new();
        let mut battery_gauge = hopspot::BatteryGauge::lipo();
        let mut persistence_notice = hopspot::PersistenceNotice::new();
        let mut controller_sleep_pending = false;
        loop {
            if controller_sleep_pending && display.deep_sleep().await.is_ok() {
                controller_sleep_pending = false;
            }
            let mut adc = [0i16; 1];
            saadc.sample(&mut adc).await;
            let vbat_mv = (adc[0].max(0) as u32) * 6000 / 4096;
            let battery = battery_gauge.update(
                Some(vbat_mv),
                hopspot::ExternalPowerState::from_presence(usb_vbus_present()),
            );

            let snapshots = build_snapshots(lora_status, usb_status);
            let mut cards = build_cards(&snapshots, lora_status.id(), usb_status.id());
            let now_ms = embassy_time::Instant::now().as_millis();
            let now = hopspot::display::MonotonicMillis::new(now_ms);
            let activity_secs = (now_ms / 1000).min(u64::from(u32::MAX)) as u32;
            activity.update(&mut cards, activity_secs);
            let content = hopspot::ScreenContent {
                cards: &cards,
                local_docs: None,
            };
            ui_state.sync(content);
            if let Some(notice) =
                persistence_notice.observe(super::learned_state::persistence_state())
            {
                show_notice(
                    &mut ui_state,
                    &mut notice_timer,
                    notice,
                    STARTUP_NOTICE_DURATION,
                );
                refresh_urgency = hopspot::display::PresentationUrgency::Immediate;
            }
            if let Some(owner) = notice_timer.expire(now) {
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
                refresh_urgency = hopspot::display::PresentationUrgency::Immediate;
            }

            let mut interface_menu_details = hopspot::ble_interface_menu_details(
                Some(super::bluetooth_auto::local_discovery_group()),
                ui_state.selected_card(content.cards),
                &snapshots,
            );
            if ui_state
                .selected_card(content.cards)
                .is_some_and(|card| card.id() == BLE_SUPERVISOR_ID)
            {
                let recovery = BluetoothAutoStatus::new(&BLE_SHARED).recovery_counters();
                interface_menu_details.push_bluetooth_recovery(
                    hopspot::BluetoothRecoveryMenuDetails {
                        receive_pressure: recovery.ingress_pressure,
                        setup_failures: recovery.setup_failures,
                        transport_closures: recovery.transport_closures,
                    },
                );
                interface_menu_details
                    .push_egress_pressure(BLE_MANIFOLD_LANE.egress_pressure_events());
            }
            if ui_state
                .selected_card(content.cards)
                .is_some_and(|card| card.id() == lora_status.id())
            {
                let spectrum = lora_spectrum.snapshot();
                interface_menu_details.push_lora_spectrum(hopspot::LoRaSpectrumMenuDetails {
                    channel_busy_per_mille: spectrum.channel_busy_per_mille,
                    noise_floor_dbm: spectrum.noise_floor_dbm,
                    cca_threshold_dbm: spectrum.cca_threshold_dbm,
                    deferrals: spectrum.deferrals,
                    false_preambles: spectrum.false_preambles,
                    contention_timeouts: spectrum.contention_timeouts,
                    duty_holds: spectrum.duty_holds,
                    duty_timeouts: spectrum.duty_timeouts,
                    radio_recoveries: spectrum.radio_recoveries,
                });
            }
            let presentation = display
                .render_and_present(
                    hopspot::face_64x128::RenderInput {
                        content,
                        battery,
                        gnss: None,
                        state: &ui_state,
                        interface_menu_details: &interface_menu_details,
                    },
                    now,
                    refresh_urgency,
                    || {
                        hopspot::display::MonotonicMillis::new(
                            embassy_time::Instant::now().as_millis(),
                        )
                    },
                )
                .await;
            let presentation_deadline = match presentation {
                Ok(RetainedPresentation::Presented | RetainedPresentation::Unchanged) => {
                    let completed_at = hopspot::display::MonotonicMillis::new(
                        embassy_time::Instant::now().as_millis(),
                    );
                    let presented_notice = notice_timer
                        .presentation_succeeded(ui_state.visible_notice(), completed_at);
                    if presented_notice == Some(hopspot::UiNotice::Sleeping) {
                        controller_sleep_pending = true;
                    }
                    refresh_urgency = hopspot::display::PresentationUrgency::Telemetry;
                    None
                }
                Ok(RetainedPresentation::DeferredUntil(deadline)) => Some(deadline),
                Ok(RetainedPresentation::Sleeping) | Err(_) => None,
            };

            match select4(
                board::INPUT_EVENTS.receive(),
                INTERFACE_STORE.changed(),
                Timer::after(STATS_POLL),
                next_deadline_timer(presentation_deadline, notice_timer.deadline()),
            )
            .await
            {
                Either4::First(first_event) => {
                    let mut next_event = Some(first_event);
                    for index in 0..board::INPUT_EVENT_CAPACITY {
                        let Some(event) = next_event.take() else {
                            break;
                        };
                        refresh_urgency = hopspot::display::PresentationUrgency::Immediate;
                        let action = ui_state.handle_input(event, content);
                        notice_timer.reconcile(ui_state.visible_notice());
                        match action {
                            hopspot::UiAction::Sleep => {
                                show_notice(
                                    &mut ui_state,
                                    &mut notice_timer,
                                    hopspot::UiNotice::Sleeping,
                                    NOTICE_DURATION,
                                );
                                lora_status.disable();
                                usb_status.disable();
                                let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                status.disable();
                            }
                            hopspot::UiAction::Wake => {
                                controller_sleep_pending = false;
                                let _ = display.wake().await;
                                show_notice(
                                    &mut ui_state,
                                    &mut notice_timer,
                                    hopspot::UiNotice::Awake,
                                    NOTICE_DURATION,
                                );
                                lora_status.enable();
                                usb_status.enable();
                                let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                status.enable();
                            }
                            hopspot::UiAction::Announce => {
                                show_notice(
                                    &mut ui_state,
                                    &mut notice_timer,
                                    hopspot::UiNotice::Announcing,
                                    NOTICE_DURATION,
                                );
                                let _ = ui_handle.issue(PrnsCommand::AnnounceNow(AnnounceNow {
                                    destination: node_page_destination,
                                    target: AnnounceTarget::AllInterfaces,
                                    app_data: AnnounceAppData::Registered,
                                }));
                            }
                            hopspot::UiAction::ToggleSelectedInterface => {
                                if let Some(card) = ui_state.selected_card(content.cards) {
                                    if card.id() == lora_status.id() {
                                        let notice = if lora_status.is_enabled() {
                                            hopspot::UiNotice::TurningOff
                                        } else {
                                            hopspot::UiNotice::TurningOn
                                        };
                                        show_notice(
                                            &mut ui_state,
                                            &mut notice_timer,
                                            notice,
                                            NOTICE_DURATION,
                                        );
                                        lora_status.toggle_enabled();
                                    } else if card.id() == usb_status.id() {
                                        let notice = if usb_status.is_enabled() {
                                            hopspot::UiNotice::TurningOff
                                        } else {
                                            hopspot::UiNotice::TurningOn
                                        };
                                        show_notice(
                                            &mut ui_state,
                                            &mut notice_timer,
                                            notice,
                                            NOTICE_DURATION,
                                        );
                                        usb_status.toggle_enabled();
                                    } else if card.id() == BLE_SUPERVISOR_ID {
                                        let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                        let notice = if status.is_enabled() {
                                            hopspot::UiNotice::TurningOff
                                        } else {
                                            hopspot::UiNotice::TurningOn
                                        };
                                        show_notice(
                                            &mut ui_state,
                                            &mut notice_timer,
                                            notice,
                                            NOTICE_DURATION,
                                        );
                                        status.toggle_enabled();
                                    }
                                }
                            }
                            hopspot::UiAction::OpenLoRaEditor => {
                                ui_state.open_lora_editor(working_lora_profile);
                            }
                            hopspot::UiAction::SetLoRaProfile(profile) => {
                                let result = hopspot::apply_and_persist_radio_profile(
                                    async {
                                        LORA_CONTROL.apply(profile).await
                                            == LoRaApplyOutcome::Applied
                                    },
                                    || async { lora_profile_store.save(profile).await.is_ok() },
                                )
                                .await;
                                if result.applied() {
                                    working_lora_profile = profile;
                                }
                                show_notice(
                                    &mut ui_state,
                                    &mut notice_timer,
                                    result.notice(),
                                    NOTICE_DURATION,
                                );
                            }
                            hopspot::UiAction::ResetLoRaProfile => {
                                let result = hopspot::apply_and_persist_radio_profile(
                                    async {
                                        LORA_CONTROL.apply(DEFAULT_915_PROFILE).await
                                            == LoRaApplyOutcome::Applied
                                    },
                                    || async { lora_profile_store.reset().await.is_ok() },
                                )
                                .await;
                                if result.applied() {
                                    working_lora_profile = DEFAULT_915_PROFILE;
                                }
                                show_notice(
                                    &mut ui_state,
                                    &mut notice_timer,
                                    result.notice(),
                                    NOTICE_DURATION,
                                );
                            }
                            hopspot::UiAction::OpenDocs => {}
                            hopspot::UiAction::SwapRadioMode => {}
                            hopspot::UiAction::ToggleStationUplink => {}
                            hopspot::UiAction::BlankDisplay => {}
                            hopspot::UiAction::ToggleDisplayAutoOff => {}
                            hopspot::UiAction::CopySharedInstanceConfig => {}
                            hopspot::UiAction::ControlGnss(_) => {}
                            hopspot::UiAction::None => {}
                        }
                        if index + 1 == board::INPUT_EVENT_CAPACITY {
                            break;
                        }
                        next_event = board::INPUT_EVENTS.try_receive().ok();
                    }
                }
                Either4::Second(()) | Either4::Third(()) | Either4::Fourth(()) => {}
            }
        }
    };

    let io = join5(
        usb_fut,
        usb_dev.run(usb_seam),
        heartbeat,
        board::drive_button(button),
        board::drive_frontlight(frontlight),
    );
    let ble_plane = async move {
        match bluetooth {
            Some((supervisor, fleet)) => {
                join3(acceptor(sd, &HUB), scanner(sd, &HUB), supervisor.run(fleet)).await;
            }
            None => core::future::pending().await,
        }
    };
    let mesh = join(lora.run(lora_seam), render);
    join3(io, ble_plane, mesh).await;
    core::future::pending().await
}
