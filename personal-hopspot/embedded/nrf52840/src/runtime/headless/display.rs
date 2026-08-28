use core::fmt::Write as _;
use core::future::Future;

#[cfg(feature = "board-t114")]
use embassy_futures::join::join5;
#[cfg(feature = "board-t096")]
use embassy_futures::join::{join3, join4};
use embassy_futures::select::{select3, Either3};
use embassy_nrf::gpio::Input;
use embassy_time::{Duration, Timer};
use personal_hopspot_core as hopspot;
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand};
use personal_rns::interfaces::lora::{RadioProfile, DEFAULT_915_PROFILE};
use personal_rns::interfaces::{
    InterfaceGravity, InterfaceId, InterfaceMode, InterfaceSnapshot, InterfaceStatus, Membership,
};
use personal_rns::lora::{LoRaApplyOutcome, LoRaSpectrumStatus};
use personal_rns::manifold::embassy::EmbassyInterfaceStatus;
use personal_rns::runtime::PrnsNodeHandle;
use personal_rns::storage::StorageLayout;
use personal_rns::wire::DestinationHash;

use crate::boards::selected as board;

use super::bluetooth::{self, BluetoothAutoStatus, BLE_SHARED, BLE_SUPERVISOR_ID, MEMBERS};
use super::{BLE_MANIFOLD_LANE, COMMANDS, COMPLETION, INTERFACE_STORE, LORA_CONTROL};

pub(super) const INTERFACE_CAPACITY: usize = 2 + MEMBERS;
pub(super) const LANE_COUNT: usize = 3;
const NOTICE_MS: u64 = 900;

fn display_now() -> hopspot::display::MonotonicMillis {
    hopspot::display::MonotonicMillis::new(embassy_time::Instant::now().as_millis())
}

pub(super) struct LoadedProfile {
    pub(super) store: ProfileStore,
    pub(super) profile: RadioProfile,
    pub(super) startup_notice: Option<hopspot::UiNotice>,
}

pub(super) struct FaceInput {
    pub(super) display: board::Display,
    pub(super) battery: board::Battery,
    pub(super) profile_store: ProfileStore,
    pub(super) identity_startup_notice: Option<hopspot::UiNotice>,
    pub(super) profile_startup_notice: Option<hopspot::UiNotice>,
    pub(super) lora_profile: RadioProfile,
    pub(super) lora_status: &'static EmbassyInterfaceStatus,
    pub(super) usb_status: &'static EmbassyInterfaceStatus,
    pub(super) lora_spectrum: &'static LoRaSpectrumStatus,
    pub(super) node_page_destination: DestinationHash,
}

type ProfileStore = hopspot::RadioProfileStore<super::super::learned_state::BoardFlash>;

pub(super) const fn heartbeat_timing() -> &'static super::super::heartbeat::HeartbeatTiming {
    &super::super::heartbeat::NORMAL
}

pub(super) async fn maintain() {
    #[cfg(feature = "board-t114")]
    board::maintain().await;
}

pub(super) async fn load_profile(
    shared_flash: super::super::learned_state::BoardFlash,
) -> LoadedProfile {
    let mut store = hopspot::RadioProfileStore::new(shared_flash, board::RADIO_PROFILE_PAGES);
    let loaded = match store.load(DEFAULT_915_PROFILE).await {
        Ok(loaded) => loaded,
        Err(_) => hopspot::LoadedRadioProfile {
            profile: DEFAULT_915_PROFILE,
            follows_default: true,
            notice: Some(hopspot::RadioProfileLoadNotice::Reset),
        },
    };
    let startup_notice = loaded.notice.map(|notice| match notice {
        hopspot::RadioProfileLoadNotice::Recovered => hopspot::UiNotice::ProfileRecovered,
        hopspot::RadioProfileLoadNotice::Reset => hopspot::UiNotice::ProfileReset,
    });
    LoadedProfile {
        store,
        profile: loaded.profile,
        startup_notice,
    }
}

pub(super) fn face(input: FaceInput) -> impl Future {
    let FaceInput {
        display,
        mut battery,
        mut profile_store,
        identity_startup_notice,
        profile_startup_notice,
        lora_profile,
        lora_status,
        usb_status,
        lora_spectrum,
        node_page_destination,
    } = input;
    let ui_handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    async move {
        let mut display = display.into_runtime(display_now());
        let mut ui_state = hopspot::UiState::new(hopspot::UiConfiguration {
            storage_limits: <board::Storage as StorageLayout>::LIMITS,
            user_blanking: display.user_blanking(),
            access_point: hopspot::AccessPointState::Unsupported,
            shared_instance_config_export: hopspot::SharedInstanceConfigExport::Unavailable,
            #[cfg(feature = "board-t096")]
            gnss: hopspot::GnssAvailability::Available,
            #[cfg(feature = "board-t114")]
            gnss: hopspot::GnssAvailability::Unavailable,
        });
        let mut activity = hopspot::CardActivityTracker::<{ MEMBERS + 4 }>::new();
        let mut battery_gauge = hopspot::BatteryGauge::lipo();
        let mut persistence_notice = hopspot::PersistenceNotice::new();
        let mut working_lora_profile = lora_profile;
        let startup_notice = identity_startup_notice.or(profile_startup_notice);
        let mut pending_startup_notice = identity_startup_notice
            .is_some()
            .then_some(profile_startup_notice)
            .flatten();
        if let Some(notice) = startup_notice {
            ui_state.show_notice(notice);
        }
        let mut notice_until_ms =
            startup_notice.map(|notice| (embassy_time::Instant::now().as_millis() + 5_000, notice));
        loop {
            let battery_mv = battery.sample_millivolts().await;
            let battery_state = battery_gauge.update(
                Some(battery_mv),
                hopspot::ExternalPowerState::from_presence(bluetooth::usb_vbus_present()),
            );
            let snapshots = snapshots(lora_status, usb_status);
            let mut cards = cards(&snapshots, lora_status.id(), usb_status.id());
            let now_ms = embassy_time::Instant::now().as_millis();
            if let Some((until, owner)) = notice_until_ms {
                if now_ms >= until {
                    notice_until_ms = None;
                    if ui_state.clear_notice_if(owner) {
                        if let Some(notice) = pending_startup_notice.take() {
                            ui_state.show_notice(notice);
                            notice_until_ms = Some((now_ms + 5_000, notice));
                        }
                    } else {
                        pending_startup_notice = None;
                    }
                }
            }
            activity.update(&mut cards, (now_ms / 1_000).min(u64::from(u32::MAX)) as u32);
            let content = hopspot::ScreenContent {
                cards: &cards,
                local_docs: None,
            };
            ui_state.sync(content);
            persistence_notice.update(
                &mut ui_state,
                super::super::learned_state::persistence_state(),
                now_ms,
            );
            let mut details = hopspot::snapshots_to_interface_menu_details(
                ui_state.selected_card(content.cards),
                &snapshots,
            );
            if ui_state
                .selected_card(content.cards)
                .is_some_and(|card| card.id() == BLE_SUPERVISOR_ID)
            {
                let recovery = BluetoothAutoStatus::new(&BLE_SHARED).recovery_counters();
                details.push_bluetooth_recovery(hopspot::BluetoothRecoveryMenuDetails {
                    receive_pressure: recovery.ingress_pressure,
                    setup_failures: recovery.setup_failures,
                    transport_closures: recovery.transport_closures,
                });
                details.push_egress_pressure(BLE_MANIFOLD_LANE.egress_pressure_events());
            }
            if ui_state
                .selected_card(content.cards)
                .is_some_and(|card| card.id() == lora_status.id())
            {
                let spectrum = lora_spectrum.snapshot();
                details.push_lora_spectrum(hopspot::LoRaSpectrumMenuDetails {
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
            let now = hopspot::display::MonotonicMillis::new(now_ms);
            let _blanking = display.poll_blanking(now, display_now).await;
            #[cfg(feature = "board-t096")]
            let gnss = (display.visibility() == hopspot::display::DisplayVisibility::Visible
                && ui_state.gnss_visible())
            .then(board::gnss_snapshot);
            #[cfg(feature = "board-t114")]
            let gnss = None;
            let _presentation = display.render_and_present(
                hopspot::face_64x128::RenderInput {
                    content,
                    battery: battery_state,
                    gnss,
                    state: &ui_state,
                    interface_menu_details: &details,
                },
                now,
                display_now,
            );
            match select3(
                board::INPUT_EVENTS.receive(),
                INTERFACE_STORE.changed(),
                Timer::after(Duration::from_secs(1)),
            )
            .await
            {
                Either3::First(event) => {
                    let now_ms = embassy_time::Instant::now().as_millis();
                    let now = hopspot::display::MonotonicMillis::new(now_ms);
                    match display.button_pressed(now, display_now).await {
                        Ok(hopspot::display::DisplayButtonOutcome::WakeAndConsume) => {
                            if display.visibility() == hopspot::display::DisplayVisibility::Visible
                            {
                                let notice = hopspot::UiNotice::Awake;
                                ui_state.show_notice(notice);
                                notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                            }
                            continue;
                        }
                        Ok(hopspot::display::DisplayButtonOutcome::ForwardToUi) => {}
                        Err(_) => continue,
                    }
                    match ui_state.handle_input(event, content) {
                        hopspot::UiAction::Announce => {
                            let notice = hopspot::UiNotice::Announcing;
                            ui_state.show_notice(notice);
                            notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                            let _issued = ui_handle.issue(PrnsCommand::AnnounceNow(AnnounceNow {
                                destination: node_page_destination,
                                target: AnnounceTarget::AllInterfaces,
                                app_data: AnnounceAppData::Registered,
                            }));
                        }
                        hopspot::UiAction::Sleep => {
                            let notice = hopspot::UiNotice::Sleeping;
                            ui_state.show_notice(notice);
                            notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                            let _scheduled = display.schedule_blanking(
                                hopspot::display::MonotonicMillis::new(
                                    now_ms.saturating_add(NOTICE_MS),
                                ),
                                hopspot::display::DisplayBlankReason::SystemSleep,
                            );
                            lora_status.disable();
                            usb_status.disable();
                            BluetoothAutoStatus::new(&BLE_SHARED).disable();
                            #[cfg(feature = "board-t096")]
                            board::control_gnss(hopspot::GnssReceiverCommand::Disable);
                        }
                        hopspot::UiAction::Wake => {
                            let _restoration = display.request_visible(now, display_now).await;
                            let notice = hopspot::UiNotice::Awake;
                            ui_state.show_notice(notice);
                            notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                            lora_status.enable();
                            usb_status.enable();
                            BluetoothAutoStatus::new(&BLE_SHARED).enable();
                            #[cfg(feature = "board-t096")]
                            if ui_state.gnss_visible() {
                                board::control_gnss(hopspot::GnssReceiverCommand::Enable);
                            }
                        }
                        hopspot::UiAction::BlankDisplay => {
                            let notice = hopspot::UiNotice::DisplayOff;
                            ui_state.show_notice(notice);
                            notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                            let _scheduled = display.schedule_blanking(
                                hopspot::display::MonotonicMillis::new(
                                    now_ms.saturating_add(NOTICE_MS),
                                ),
                                hopspot::display::DisplayBlankReason::DisplayOnly,
                            );
                        }
                        hopspot::UiAction::ToggleDisplayAutoOff => {
                            if let Ok(auto_off) = display.toggle_auto_off(now) {
                                let notice = match auto_off {
                                    hopspot::display::DisplayAutoOff::Enabled => {
                                        hopspot::UiNotice::DisplayAutoOffOn
                                    }
                                    hopspot::display::DisplayAutoOff::Disabled => {
                                        hopspot::UiNotice::DisplayAutoOffOff
                                    }
                                };
                                ui_state.show_notice(notice);
                                notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                            }
                        }
                        #[cfg(feature = "board-t096")]
                        hopspot::UiAction::ControlGnss(command) => board::control_gnss(command),
                        #[cfg(feature = "board-t114")]
                        hopspot::UiAction::ControlGnss(_) => {}
                        hopspot::UiAction::ToggleSelectedInterface => {
                            if let Some(card) = ui_state.selected_card(content.cards) {
                                let status = if card.id() == lora_status.id() {
                                    Some(lora_status)
                                } else if card.id() == usb_status.id() {
                                    Some(usb_status)
                                } else if card.id() == BLE_SUPERVISOR_ID {
                                    let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                    let notice = if status.is_enabled() {
                                        hopspot::UiNotice::TurningOff
                                    } else {
                                        hopspot::UiNotice::TurningOn
                                    };
                                    ui_state.show_notice(notice);
                                    notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                                    status.toggle_enabled();
                                    None
                                } else {
                                    None
                                };
                                if let Some(status) = status {
                                    let notice = if status.is_enabled() {
                                        hopspot::UiNotice::TurningOff
                                    } else {
                                        hopspot::UiNotice::TurningOn
                                    };
                                    ui_state.show_notice(notice);
                                    notice_until_ms = Some((now_ms + NOTICE_MS, notice));
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
                                || async { profile_store.save(profile).await.is_ok() },
                            )
                            .await;
                            if result.applied() {
                                working_lora_profile = profile;
                            }
                            let notice = result.notice();
                            ui_state.show_notice(notice);
                            notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                        }
                        hopspot::UiAction::ResetLoRaProfile => {
                            let result = hopspot::apply_and_persist_radio_profile(
                                async {
                                    LORA_CONTROL.apply(DEFAULT_915_PROFILE).await
                                        == LoRaApplyOutcome::Applied
                                },
                                || async { profile_store.reset().await.is_ok() },
                            )
                            .await;
                            if result.applied() {
                                working_lora_profile = DEFAULT_915_PROFILE;
                            }
                            let notice = result.notice();
                            ui_state.show_notice(notice);
                            notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                        }
                        hopspot::UiAction::None
                        | hopspot::UiAction::ToggleStationUplink
                        | hopspot::UiAction::SwapRadioMode
                        | hopspot::UiAction::OpenDocs
                        | hopspot::UiAction::CopySharedInstanceConfig => {}
                    }
                }
                Either3::Second(()) | Either3::Third(()) => {}
            }
        }
    }
}

#[cfg(feature = "board-t114")]
pub(super) fn run<I, L, F, B>(
    io: I,
    lora: L,
    face: F,
    bluetooth: B,
    button: Input<'static>,
) -> impl Future
where
    I: Future,
    L: Future,
    F: Future,
    B: Future,
{
    join5(io, lora, face, bluetooth, board::drive_button(button))
}

#[cfg(feature = "board-t096")]
pub(super) fn run<I, L, F, B>(
    io: I,
    lora: L,
    face: F,
    bluetooth: B,
    button: Input<'static>,
    gnss: board::Gnss,
) -> impl Future
where
    I: Future,
    L: Future,
    F: Future,
    B: Future,
{
    let primary = join4(io, lora, face, board::drive_button(button));
    join3(primary, board::drive_gnss(gnss), bluetooth)
}

fn snapshots(
    lora: &EmbassyInterfaceStatus,
    usb: &EmbassyInterfaceStatus,
) -> heapless::Vec<InterfaceSnapshot, { MEMBERS + 4 }> {
    let ble = BluetoothAutoStatus::new(&BLE_SHARED);
    let mut entries: heapless::Vec<(&dyn InterfaceStatus, Membership), { MEMBERS + 4 }> =
        heapless::Vec::new();
    let _ = entries.push((lora, Membership::Independent));
    let _ = entries.push((usb, Membership::Independent));
    let supervisor_id = ble.id();
    let _ = entries.push((&ble, Membership::Independent));
    for member in ble.members() {
        let _ = entries.push((member, Membership::FleetMember { supervisor_id }));
    }
    let mut snapshots = heapless::Vec::new();
    for (status, membership) in &entries {
        let counts = INTERFACE_STORE.counts(status.id());
        let _ = snapshots.push(InterfaceSnapshot {
            id: status.id(),
            mode: InterfaceMode::Full,
            gravity: InterfaceGravity::ZERO,
            connection: status.connection(),
            failure_reason: status.failure_reason(),
            rx_bytes: status.rx_bytes(),
            tx_bytes: status.tx_bytes(),
            transfer_rates: status.transfer_rates(),
            destinations: counts.destinations,
            links: counts.links,
            transported_links: counts.transported_links,
            membership: *membership,
        });
    }
    snapshots
}

fn cards(
    snapshots: &[InterfaceSnapshot],
    lora_id: InterfaceId,
    usb_id: InterfaceId,
) -> heapless::Vec<hopspot::Card, { MEMBERS + 4 }> {
    hopspot::snapshots_to_cards(snapshots, |id| {
        if id == lora_id {
            Some((hopspot::CardKind::LoRa, hopspot::card_label("LoRa")))
        } else if id == usb_id {
            Some((hopspot::CardKind::Usb, hopspot::card_label("USB")))
        } else if id == BLE_SUPERVISOR_ID {
            Some((hopspot::CardKind::Ble, hopspot::card_label("BLE")))
        } else {
            let bytes = id.as_bytes();
            let mut label = hopspot::CardLabel::new();
            let _ = write!(label, "Peer {:02x}{:02x}", bytes[1], bytes[2]);
            Some((hopspot::CardKind::Peer, label))
        }
    })
}
