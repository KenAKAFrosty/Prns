use embassy_executor::Spawner;
use embassy_futures::join::{join, join3, join5};
use embassy_futures::select::{select3, Either3};
use embassy_nrf::gpio::{Input, Output};
use embassy_nrf::spim::Spim;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Delay, Duration, Timer};
use embassy_usb::{Builder, Config as UsbConfig};
use static_cell::{ConstStaticCell, StaticCell};

use embedded_graphics::prelude::*;
use embedded_hal_bus::spi::ExclusiveDevice;
use epd_waveshare::color::Color as EpdColor;

use nrf_softdevice::ble::l2cap;
use nrf_softdevice::{Flash, Softdevice};

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
use personal_rns::runtime::{
    Fleet, PrnsEvent, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe, SharedNorFlash,
};
use personal_rns::storage::StorageLayout;
use personal_rns::usb_auto::{UsbAutoDevice, UsbAutoDeviceInput};
use personal_rns::usb_auto::{WebUsbAutoClass, WebUsbAutoState, WEBUSB_AUTO_PACKET_SIZE};

use super::bluetooth_auto::{
    acceptor, scanner, serve_slot, softdevice_config, softdevice_task, usb_vbus_present,
    L2capPacket, NrfBleBackend, Server, BLE_SHARED, BLE_SUPERVISOR_ID, HUB, MEMBERS, OUTBOUND_WAKE,
    POOL,
};
use super::board::{
    TechoBoard, TechoControls, TechoDisplayHardware, TechoEarlyHardware, TechoFaceHardware,
    TechoRuntimeHardware, TechoUsbHardware,
};
use super::display::{build_cards, build_snapshots, frame_hash, EinkScreen};
use super::input;
use super::node::*;

const PARTIAL_REFRESH_LIMIT: u32 = 64;
const FULL_REFRESH_MAX_AGE_MS: u64 = 30 * 60 * 1_000;
const TELEMETRY_MIN_INTERVAL_MS: u64 = 5_000;
const STATS_POLL: Duration = Duration::from_secs(1);
const EINK_ANIMATION_MS: u64 = 0;
const NOTICE_MS: u64 = 900;

#[embassy_executor::task]
async fn manifold_task(
    node: &'static mut Node,
    persistence: &'static mut super::persistence::TechoPersistence,
) {
    let _ = node.restore_embedded_persistence(persistence).await;
    node.run_manifold_with_persistence_and_interface_store(&INTERFACE_STORE, persistence)
        .await
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run(spawner: Spawner) -> ! {
    let ((node_bootstrap, ble_bootstrap), early_hardware) =
        TechoBoard::initialize_identities(|nvmc, rng| {
            let mut fill_entropy = |bytes: &mut [u8]| rng.blocking_fill_bytes(bytes);
            let node_bootstrap = super::identity::bootstrap_node_identity(nvmc, &mut fill_entropy);
            let ble_bootstrap = super::identity::bootstrap_ble_identity(nvmc, &mut fill_entropy);
            (node_bootstrap, ble_bootstrap)
        });
    let identity_startup_notice =
        super::identity::startup_notice(node_bootstrap.persistence(), ble_bootstrap.persistence());
    let node_identity = node_bootstrap.into_identity();
    let ble_identity = Some(ble_bootstrap.into_identity());

    let TechoEarlyHardware {
        usb,
        face,
        deferred,
    } = early_hardware;
    let TechoUsbHardware {
        driver: usb_driver,
        vbus,
    } = usb;
    let mut usb_config = UsbConfig::new(WEBUSB_VENDOR_ID, WEBUSB_PRODUCT_ID);
    usb_config.manufacturer = Some("Stay Personal");
    usb_config.product = Some("Personal Hopspot (T-Echo)");
    usb_config.serial_number = Some("PERSONAL-RNS-TECHO-HOP");
    usb_config.max_packet_size_0 = 64;
    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    let mut builder = Builder::new(
        usb_driver,
        usb_config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 256]),
        CONTROL_BUF.init([0; 64]),
    );
    builder.msos_descriptor(0x0603_0000, 0x20);
    static USB_STATE: StaticCell<WebUsbAutoState> = StaticCell::new();
    let class = WebUsbAutoClass::new(
        &mut builder,
        USB_STATE.init(WebUsbAutoState::new()),
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
    let flash = Flash::take(sd);
    static FLASH_STORAGE: StaticCell<Mutex<CriticalSectionRawMutex, Flash>> = StaticCell::new();
    let flash = FLASH_STORAGE.init(Mutex::new(flash));
    let shared_flash = SharedNorFlash::new(flash, 1024 * 1024);
    let mut lora_profile_store =
        hopspot::RadioProfileStore::new(shared_flash, hopspot::T_ECHO_RADIO_PROFILE_PAGES);
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
        super::bluetooth_auto::set_columba_identity(server, identity);
    }
    if ble_identity.is_some() {
        for idx in 0..POOL {
            spawner.spawn(serve_slot(idx, sd, l2cap, server, &HUB).expect("serve slot fits"));
        }
    }

    let TechoRuntimeHardware {
        radio,
        display,
        controls,
    } = deferred.finish().await;
    let TechoDisplayHardware {
        driver: eink,
        mut panel,
        _rail: _eink_rail,
    } = display;
    let TechoControls { button, frontlight } = controls;
    let TechoFaceHardware {
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
    let self_destination = destination_hashes.delivery;
    let node_page_destination = destination_hashes.node_page;
    let seed = self_destination.as_bytes();
    ENTROPY_STATE.store(
        u32::from_le_bytes([seed[0], seed[1], seed[2], seed[3]]) | 1,
        core::sync::atomic::Ordering::Relaxed,
    );
    let mut manifold_lanes = ManifoldLanes::new();
    let lora_profile = loaded_lora_profile.profile;
    let lora_id = LoRaInterface::<
        ExclusiveDevice<Spim<'static>, Output<'static>, Delay>,
        Input<'static>,
        Input<'static>,
        Output<'static>,
        Delay,
    >::interface_id(&lora_profile);
    static LORA_STATUS: StaticCell<EmbassyInterfaceStatus> = StaticCell::new();
    let lora_status: &'static EmbassyInterfaceStatus = LORA_STATUS.init(
        EmbassyInterfaceStatus::new(lora_id, ConnectionState::Initializing),
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
    let usb_status: &'static EmbassyInterfaceStatus = USB_STATUS.init(EmbassyInterfaceStatus::new(
        USB_INTERFACE_ID,
        ConnectionState::Initializing,
    ));
    let usb_dev = UsbAutoDevice::new(UsbAutoDeviceInput {
        rx: usb_rx,
        tx: usb_tx,
        status: usb_status,
        host_present: || true,
    });

    let lora_lane = manifold_lanes
        .claim_interface(&LORA_MANIFOLD_LANE, lora.descriptor())
        .expect("LoRa lane is available");
    let ble_supervisor_lane = ble_identity.as_ref().map(|_| {
        manifold_lanes
            .claim_supervisor(&BLE_MANIFOLD_LANE, BLE_SUPERVISOR_ID, &OUTBOUND_WAKE)
            .expect("Bluetooth supervisor lane is available")
    });
    let usb_lane = manifold_lanes
        .claim_interface(&USB_MANIFOLD_LANE, usb_dev.descriptor())
        .expect("USB lane is available");

    let handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let manifold_wiring = manifold_lanes.into_manifold_wiring(
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new(seeded_entropy as fn(&mut [u8]));
    static NODE: StaticCell<Node> = StaticCell::new();
    let recipe = PrnsNodeRecipe {
        transport_identity: Some(transport_secret),
        pre_configured_destinations: hopspot::HopspotDestinationSet::new(
            destination_secret,
            ANNOUNCE_APP_DATA,
            NODE_ANNOUNCE_APP_DATA,
        )
        .into_preconfigured_destinations(),
        app_state: (),
        storage: crate::storage::TechoStorage,
        request_endpoints: hopspot::node_pages::NodePageRoutes,
        interfaces: personal_rns::runtime::ManuallyAttached,
        persistence: super::persistence::new(shared_flash),
        on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
    };
    let (node, persistence) =
        PrnsNode::init_static_with_persistence(&NODE, recipe, manifold_wiring, host);
    static PERSISTENCE: StaticCell<super::persistence::TechoPersistence> = StaticCell::new();
    let persistence = PERSISTENCE.init(persistence);
    spawner.spawn(manifold_task(node, persistence).expect("manifold task fits"));
    let lora_seam = lora_lane.into_seam(NOTIFY.sender(), seeded_entropy);

    let usb_seam = usb_lane.into_seam(NOTIFY.sender(), seeded_entropy);

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
                &BLE_SHARED,
            );
            let fleet: Fleet<Mtx, BLE_HW_MTU, NOTIFY_CAP, LIFECYCLE_CAP> =
                lane.into_fleet(NOTIFY.sender(), LIFECYCLE.sender());
            (supervisor, fleet)
        });

    let usb_fut = usb.run();

    let heartbeat = async {
        let mut n = 0u32;
        loop {
            Timer::after(Duration::from_secs(1)).await;
            n = n.wrapping_add(1);
            if n & 1 == 0 {
                led.set_low();
            } else {
                led.set_high();
            }
        }
    };

    let ui_handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let render = async move {
        let mut saadc = saadc;
        let mut epd = match eink {
            Some(epd) => epd,
            None => core::future::pending().await,
        };
        let mut ui_state = hopspot::UiState::new(hopspot::UiConfiguration {
            storage_limits: <crate::storage::TechoStorage as StorageLayout>::LIMITS,
            display_power_control: hopspot::DisplayPowerControl::Unavailable,
            access_point: hopspot::AccessPointState::Unsupported,
        });
        let startup_notice = identity_startup_notice.or(profile_startup_notice);
        let mut pending_startup_notice = identity_startup_notice
            .is_some()
            .then_some(profile_startup_notice)
            .flatten();
        if let Some(notice) = startup_notice {
            ui_state.show_notice(notice);
        }
        let mut working_lora_profile = lora_profile;
        let mut refresh_policy = hopspot::EinkRefreshPolicy::new(
            PARTIAL_REFRESH_LIMIT,
            FULL_REFRESH_MAX_AGE_MS,
            TELEMETRY_MIN_INTERVAL_MS,
        );
        let mut refresh_urgency = hopspot::EinkRefreshUrgency::Immediate;
        let mut displayed_hash = None;
        let mut activity = hopspot::CardActivityTracker::<{ MEMBERS + 4 }>::new();
        let mut notice_until_ms =
            startup_notice.map(|_| embassy_time::Instant::now().as_millis() + 5_000);
        let mut battery_gauge = hopspot::BatteryGauge::lipo();
        let mut persistence_notice_visible = false;
        loop {
            let mut adc = [0i16; 1];
            saadc.sample(&mut adc).await;
            let vbat_mv = (adc[0].max(0) as u32) * 6000 / 4096;
            let battery = battery_gauge.update(Some(vbat_mv), usb_vbus_present());

            let snapshots = build_snapshots(lora_status, usb_status);
            let mut cards = build_cards(&snapshots, lora_status.id(), usb_status.id());
            let now_ms = embassy_time::Instant::now().as_millis();
            let activity_secs = (now_ms / 1000).min(u64::from(u32::MAX)) as u32;
            activity.update(&mut cards, activity_secs);
            let content = hopspot::ScreenContent {
                cards: &cards,
                local_docs: None,
            };
            ui_state.sync(content);
            let state_not_saved = super::persistence::state_not_saved();
            if state_not_saved {
                ui_state.show_notice(hopspot::UiNotice::StateNotSaved);
                notice_until_ms = None;
                persistence_notice_visible = true;
            } else if persistence_notice_visible {
                ui_state.clear_notice();
                persistence_notice_visible = false;
            }
            if notice_until_ms.is_some_and(|until| now_ms >= until) {
                if let Some(notice) = pending_startup_notice.take() {
                    ui_state.show_notice(notice);
                    notice_until_ms = Some(now_ms + 5_000);
                } else {
                    ui_state.clear_notice();
                    notice_until_ms = None;
                }
                refresh_urgency = hopspot::EinkRefreshUrgency::Immediate;
            }

            let _ = panel.clear(EpdColor::White);
            let mut interface_menu_details = hopspot::snapshots_to_interface_menu_details(
                ui_state.selected_card(content.cards),
                &snapshots,
            );
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
            hopspot::render(
                &mut EinkScreen { panel: &mut panel },
                hopspot::RenderFrame {
                    content,
                    battery,
                    state: &ui_state,
                    interface_menu_details: &interface_menu_details,
                    animation_ms: EINK_ANIMATION_MS,
                },
            );
            let hash = frame_hash(panel.buffer());
            if displayed_hash != Some(hash) {
                match refresh_policy.for_changed_frame(now_ms, &refresh_urgency) {
                    hopspot::EinkRefresh::Deferred => {}
                    hopspot::EinkRefresh::Full => {
                        if epd.full_update(panel.buffer()).is_ok() {
                            refresh_policy.full_refresh_succeeded(now_ms);
                            displayed_hash = Some(hash);
                        } else {
                            refresh_policy.refresh_failed();
                        }
                    }
                    hopspot::EinkRefresh::Partial => {
                        if epd.partial_update(panel.buffer()).is_ok() {
                            refresh_policy.partial_refresh_succeeded(now_ms);
                            displayed_hash = Some(hash);
                        } else {
                            refresh_policy.refresh_failed();
                        }
                    }
                }
            }

            match select3(
                input::EVENTS.receive(),
                INTERFACE_STORE.changed(),
                Timer::after(STATS_POLL),
            )
            .await
            {
                Either3::First(event) => {
                    refresh_urgency = hopspot::EinkRefreshUrgency::Immediate;
                    let action = ui_state.handle_input(event, content);
                    match action {
                        hopspot::UiAction::Sleep => {
                            ui_state.show_notice(hopspot::UiNotice::Sleeping);
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                            lora_status.disable();
                            usb_status.disable();
                            let status = BluetoothAutoStatus::new(&BLE_SHARED);
                            status.disable();
                        }
                        hopspot::UiAction::Wake => {
                            ui_state.show_notice(hopspot::UiNotice::Awake);
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                            lora_status.enable();
                            usb_status.enable();
                            let status = BluetoothAutoStatus::new(&BLE_SHARED);
                            status.enable();
                        }
                        hopspot::UiAction::Announce => {
                            ui_state.show_notice(hopspot::UiNotice::Announcing);
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                            let _ = ui_handle.issue(PrnsCommand::AnnounceNow(AnnounceNow {
                                destination: self_destination,
                                target: AnnounceTarget::AllInterfaces,
                                app_data: AnnounceAppData::Registered,
                            }));
                            let _ = ui_handle.issue(PrnsCommand::AnnounceNow(AnnounceNow {
                                destination: node_page_destination,
                                target: AnnounceTarget::AllInterfaces,
                                app_data: AnnounceAppData::Registered,
                            }));
                        }
                        hopspot::UiAction::ToggleSelectedInterface => {
                            if let Some(card) = ui_state.selected_card(content.cards) {
                                if card.id() == lora_status.id() {
                                    ui_state.show_notice(if lora_status.is_enabled() {
                                        hopspot::UiNotice::TurningOff
                                    } else {
                                        hopspot::UiNotice::TurningOn
                                    });
                                    notice_until_ms =
                                        Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                                    lora_status.toggle_enabled();
                                } else if card.id() == usb_status.id() {
                                    ui_state.show_notice(if usb_status.is_enabled() {
                                        hopspot::UiNotice::TurningOff
                                    } else {
                                        hopspot::UiNotice::TurningOn
                                    });
                                    notice_until_ms =
                                        Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                                    usb_status.toggle_enabled();
                                } else if card.id() == BLE_SUPERVISOR_ID {
                                    let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                    ui_state.show_notice(if status.is_enabled() {
                                        hopspot::UiNotice::TurningOff
                                    } else {
                                        hopspot::UiNotice::TurningOn
                                    });
                                    notice_until_ms =
                                        Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
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
                                    LORA_CONTROL.apply(profile).await == LoRaApplyOutcome::Applied
                                },
                                || async { lora_profile_store.save(profile).await.is_ok() },
                            )
                            .await;
                            if result.applied() {
                                working_lora_profile = profile;
                            }
                            ui_state.show_notice(result.notice());
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
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
                            ui_state.show_notice(result.notice());
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                        }
                        hopspot::UiAction::OpenDocs => {}
                        hopspot::UiAction::SwapRadioMode => {}
                        hopspot::UiAction::OledOff => {}
                        hopspot::UiAction::None => {}
                    }
                }
                Either3::Second(()) => {
                    refresh_urgency = hopspot::EinkRefreshUrgency::Immediate;
                }
                Either3::Third(()) => {
                    refresh_urgency = hopspot::EinkRefreshUrgency::Telemetry;
                }
            }
        }
    };

    let io = join5(
        usb_fut,
        usb_dev.run(usb_seam),
        heartbeat,
        input::drive_button(button),
        input::drive_frontlight(frontlight),
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
