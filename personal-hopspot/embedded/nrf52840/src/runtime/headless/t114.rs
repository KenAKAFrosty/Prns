use core::future::Future;

use embassy_futures::join::join4;
use embassy_futures::select::{select3, Either3};
use embassy_nrf::gpio::Input;
use embassy_nrf::nvmc::Nvmc;
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

use super::{COMMANDS, COMPLETION, INTERFACE_STORE, LORA_CONTROL};

pub(super) const INTERFACE_CAPACITY: usize = 2;
pub(super) const LANE_COUNT: usize = INTERFACE_CAPACITY;

pub(super) struct LoadedProfile {
    pub(super) store: board::ProfileStore,
    pub(super) profile: RadioProfile,
    pub(super) startup_notice: Option<hopspot::UiNotice>,
}

pub(super) struct FaceInput {
    pub(super) display: board::Display,
    pub(super) battery: board::Battery,
    pub(super) profile_store: board::ProfileStore,
    pub(super) identity_startup_notice: Option<hopspot::UiNotice>,
    pub(super) profile_startup_notice: Option<hopspot::UiNotice>,
    pub(super) lora_profile: RadioProfile,
    pub(super) lora_status: &'static EmbassyInterfaceStatus,
    pub(super) usb_status: &'static EmbassyInterfaceStatus,
    pub(super) lora_spectrum: &'static LoRaSpectrumStatus,
    pub(super) node_page_destination: DestinationHash,
}

pub(super) const fn heartbeat_illuminated_ms() -> u64 {
    100
}

pub(super) async fn maintain() {
    board::maintain().await;
}

pub(super) async fn load_profile(flash: Nvmc<'static>) -> LoadedProfile {
    let mut store = board::new_profile_store(flash);
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
        mut display,
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
        if !display.is_initialized() {
            core::future::pending::<()>().await;
        }
        let mut ui_state = hopspot::UiState::new(hopspot::UiConfiguration {
            storage_limits: <board::Storage as StorageLayout>::LIMITS,
            display_power_control: hopspot::DisplayPowerControl::Unavailable,
            access_point: hopspot::AccessPointState::Unsupported,
            shared_instance_config_export: hopspot::SharedInstanceConfigExport::Unavailable,
            gnss: hopspot::GnssAvailability::Unavailable,
        });
        let mut activity = hopspot::CardActivityTracker::<INTERFACE_CAPACITY>::new();
        let mut battery_gauge = hopspot::BatteryGauge::lipo();
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
                hopspot::ExternalPowerState::from_presence(board::usb_vbus_present()),
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
            let mut details = hopspot::snapshots_to_interface_menu_details(
                ui_state.selected_card(content.cards),
                &snapshots,
            );
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
            hopspot::render(
                &mut display,
                hopspot::RenderFrame {
                    content,
                    battery: battery_state,
                    gnss: None,
                    state: &ui_state,
                    interface_menu_details: &details,
                    animation_ms: now_ms,
                },
            );
            let _panel_update = display.flush();
            match select3(
                board::INPUT_EVENTS.receive(),
                INTERFACE_STORE.changed(),
                Timer::after(Duration::from_secs(1)),
            )
            .await
            {
                Either3::First(event) => {
                    let now_ms = embassy_time::Instant::now().as_millis();
                    match ui_state.handle_input(event, content) {
                        hopspot::UiAction::Announce => {
                            let notice = hopspot::UiNotice::Announcing;
                            ui_state.show_notice(notice);
                            notice_until_ms = Some((now_ms + 900, notice));
                            let _issued = ui_handle.issue(PrnsCommand::AnnounceNow(AnnounceNow {
                                destination: node_page_destination,
                                target: AnnounceTarget::AllInterfaces,
                                app_data: AnnounceAppData::Registered,
                            }));
                        }
                        hopspot::UiAction::Sleep => {
                            let notice = hopspot::UiNotice::Sleeping;
                            ui_state.show_notice(notice);
                            notice_until_ms = Some((now_ms + 900, notice));
                            lora_status.disable();
                            usb_status.disable();
                        }
                        hopspot::UiAction::Wake => {
                            let notice = hopspot::UiNotice::Awake;
                            ui_state.show_notice(notice);
                            notice_until_ms = Some((now_ms + 900, notice));
                            lora_status.enable();
                            usb_status.enable();
                        }
                        hopspot::UiAction::ToggleSelectedInterface => {
                            if let Some(card) = ui_state.selected_card(content.cards) {
                                let status = if card.id() == lora_status.id() {
                                    Some(lora_status)
                                } else if card.id() == usb_status.id() {
                                    Some(usb_status)
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
                                    notice_until_ms = Some((now_ms + 900, notice));
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
                            notice_until_ms = Some((now_ms + 900, notice));
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
                            notice_until_ms = Some((now_ms + 900, notice));
                        }
                        hopspot::UiAction::None
                        | hopspot::UiAction::OledOff
                        | hopspot::UiAction::ToggleOledAutoOff
                        | hopspot::UiAction::ControlGnss(_)
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

pub(super) fn run<I, L, F>(io: I, lora: L, face: F, button: Input<'static>) -> impl Future
where
    I: Future,
    L: Future,
    F: Future,
{
    join4(io, lora, face, board::drive_button(button))
}

fn snapshots(
    lora: &EmbassyInterfaceStatus,
    usb: &EmbassyInterfaceStatus,
) -> heapless::Vec<InterfaceSnapshot, INTERFACE_CAPACITY> {
    let entries: [(&dyn InterfaceStatus, Membership); INTERFACE_CAPACITY] = [
        (lora, Membership::Independent),
        (usb, Membership::Independent),
    ];
    let mut snapshots = heapless::Vec::new();
    for (status, membership) in entries {
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
            membership,
        });
    }
    snapshots
}

fn cards(
    snapshots: &[InterfaceSnapshot],
    lora_id: InterfaceId,
    usb_id: InterfaceId,
) -> heapless::Vec<hopspot::Card, INTERFACE_CAPACITY> {
    hopspot::snapshots_to_cards(snapshots, |id| {
        if id == lora_id {
            Some((hopspot::CardKind::LoRa, hopspot::card_label("LoRa")))
        } else if id == usb_id {
            Some((hopspot::CardKind::Usb, hopspot::card_label("USB")))
        } else {
            None
        }
    })
}
