use core::fmt::Write as _;
use core::future::Future;

#[cfg(feature = "board-t114")]
use embassy_futures::join::join5;
#[cfg(feature = "board-t096")]
use embassy_futures::join::{join3, join4};
use embassy_futures::select::{select4, Either4};
use embassy_nrf::gpio::Input;
use embassy_time::{Duration, Timer};
use personal_hopspot_core as hopspot;
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand};
use personal_rns::interfaces::subghz::SubGConfigurationState;
use personal_rns::interfaces::{
    ConnectionState, InterfaceGravity, InterfaceId, InterfaceMode, InterfaceSnapshot,
    InterfaceStatus, Membership,
};
use personal_rns::lora::{LoRaController, LoRaSpectrumStatus};
use personal_rns::manifold::embassy::EmbassyInterfaceStatus;
use personal_rns::runtime::PrnsNodeHandle;
use personal_rns::storage::StorageLayout;
use personal_rns::wire::DestinationHash;

use crate::boards::selected as board;

use super::bluetooth::{self, BluetoothAutoStatus, BLE_SHARED, BLE_SUPERVISOR_ID, MEMBERS};
use super::{BLE_MANIFOLD_LANE, COMMANDS, COMPLETION, INTERFACE_STORE, REMOTE_CONTROL_COMMANDS};

pub(super) const INTERFACE_CAPACITY: usize = 2 + MEMBERS;
pub(super) const LANE_COUNT: usize = 3;
const NOTICE_MS: u64 = 900;

fn display_now() -> hopspot::display::MonotonicMillis {
    hopspot::display::MonotonicMillis::new(embassy_time::Instant::now().as_millis())
}

pub(super) struct LoadedSubGConfiguration {
    pub(super) store: ConfigurationStore,
    pub(super) state: SubGConfigurationState,
    pub(super) startup_notice: Option<hopspot::UiNotice>,
}

pub(super) struct FaceInput {
    pub(super) display: board::Display,
    pub(super) battery: board::Battery,
    pub(super) subg_configuration_store: ConfigurationStore,
    pub(super) identity_startup_notice: Option<hopspot::UiNotice>,
    pub(super) subg_startup_notice: Option<hopspot::UiNotice>,
    pub(super) subg_configuration: SubGConfigurationState,
    pub(super) lora_status: &'static EmbassyInterfaceStatus,
    pub(super) usb_status: &'static EmbassyInterfaceStatus,
    pub(super) lora_spectrum: &'static LoRaSpectrumStatus,
    pub(super) lora_controller: LoRaController<'static>,
    pub(super) node_page_destination: DestinationHash,
}

type ConfigurationStore = hopspot::SubGConfigurationStore<super::super::learned_state::BoardFlash>;

pub(super) const fn heartbeat_timing() -> &'static super::super::heartbeat::HeartbeatTiming {
    &super::super::heartbeat::NORMAL
}

pub(super) async fn maintain() {
    #[cfg(feature = "board-t114")]
    board::maintain().await;
}

pub(super) async fn load_subg_configuration(
    shared_flash: super::super::learned_state::BoardFlash,
) -> LoadedSubGConfiguration {
    let mut store = hopspot::SubGConfigurationStore::new(shared_flash, board::RADIO_PROFILE_PAGES);
    let loaded = match store.load().await {
        Ok(loaded) => loaded,
        Err(_) => hopspot::LoadedSubGConfiguration {
            state: SubGConfigurationState::Unconfigured,
            notice: Some(hopspot::SubGConfigurationLoadNotice::Reset),
        },
    };
    let startup_notice = loaded.notice.map(|notice| match notice {
        hopspot::SubGConfigurationLoadNotice::Migrated => hopspot::UiNotice::SubGMigrated,
        hopspot::SubGConfigurationLoadNotice::Recovered => hopspot::UiNotice::SubGRecovered,
        hopspot::SubGConfigurationLoadNotice::Reset => hopspot::UiNotice::SubGReset,
    });
    LoadedSubGConfiguration {
        store,
        state: loaded.state,
        startup_notice,
    }
}

pub(super) fn face(input: FaceInput) -> impl Future {
    let FaceInput {
        display,
        mut battery,
        mut subg_configuration_store,
        identity_startup_notice,
        subg_startup_notice,
        subg_configuration,
        lora_status,
        usb_status,
        lora_spectrum,
        mut lora_controller,
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
            discovery_groups: hopspot::DiscoveryGroupEditorAvailability::Available,
            #[cfg(feature = "board-t096")]
            gnss: hopspot::GnssAvailability::Available,
            #[cfg(feature = "board-t114")]
            gnss: hopspot::GnssAvailability::Unavailable,
        });
        let mut activity = hopspot::CardActivityTracker::<{ MEMBERS + 4 }>::new();
        let mut battery_gauge = hopspot::BatteryGauge::lipo();
        let mut persistence_notice = hopspot::PersistenceNotice::new();
        let mut working_subg_configuration = subg_configuration;
        let startup_notice = identity_startup_notice.or(subg_startup_notice);
        let mut pending_startup_notice = identity_startup_notice
            .is_some()
            .then_some(subg_startup_notice)
            .flatten();
        if let Some(notice) = startup_notice {
            ui_state.show_notice(notice);
        }
        let mut notice_until_ms =
            startup_notice.map(|notice| (embassy_time::Instant::now().as_millis() + 5_000, notice));
        let mut scheduled_remote_control_effect = None;
        let mut system = super::remote_control::SystemIntent::from_status(lora_status, usb_status);
        #[cfg(feature = "board-t096")]
        let mut gnss_wanted = false;
        macro_rules! execute_hopspot_command {
            ($snapshots:expr, $power:expr, $command:expr) => {
                super::remote_control::execute(
                    super::remote_control::Context {
                        snapshots: &$snapshots,
                        lora_status,
                        usb_status,
                        display: &mut display,
                        power: $power,
                        system: &mut system,
                        scheduled_effect: &mut scheduled_remote_control_effect,
                        lora_controller: &mut lora_controller,
                        subg_store: &mut subg_configuration_store,
                        subg_configuration: &mut working_subg_configuration,
                        #[cfg(feature = "board-t096")]
                        gnss_wanted: &mut gnss_wanted,
                    },
                    $command,
                )
                .await
            };
        }
        loop {
            super::remote_control::apply_scheduled(
                &mut scheduled_remote_control_effect,
                lora_status,
                usb_status,
                &mut display,
                &mut system,
            )
            .await;
            let battery_mv = battery.sample_millivolts().await;
            let battery_state = battery_gauge.update(
                Some(battery_mv),
                hopspot::ExternalPowerState::from_presence(bluetooth::usb_vbus_present()),
            );
            let (snapshots, snapshot_failed) = match snapshots(lora_status, usb_status) {
                Ok(snapshots) => (snapshots, false),
                Err(SnapshotBuildError::CapacityExhausted) => (heapless::Vec::new(), true),
            };
            let mut cards = cards(
                &snapshots,
                working_subg_configuration,
                lora_status.id(),
                usb_status.id(),
            );
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
            match select4(
                board::INPUT_EVENTS.receive(),
                INTERFACE_STORE.changed(),
                Timer::after(Duration::from_secs(1)),
                REMOTE_CONTROL_COMMANDS.receive(),
            )
            .await
            {
                Either4::Fourth(pending) => {
                    let (token, command) = pending.into_parts();
                    let result = if snapshot_failed {
                        Err(personal_rns::runtime::RemoteControlHostCommandError::ApplyFailed)
                    } else {
                        execute_hopspot_command!(snapshots, battery_state, command)
                    };
                    REMOTE_CONTROL_COMMANDS.complete(token, result);
                }
                Either4::First(event) => {
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
                            let _result = execute_hopspot_command!(
                                snapshots,
                                battery_state,
                                personal_rns::runtime::RemoteControlHostCommand::SetSystemPower {
                                    power: personal_rns::remote_control::RemoteControlSystemPower::Asleep,
                                }
                            );
                        }
                        hopspot::UiAction::Wake => {
                            let notice = hopspot::UiNotice::Awake;
                            ui_state.show_notice(notice);
                            notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                            let _result = execute_hopspot_command!(
                                snapshots,
                                battery_state,
                                personal_rns::runtime::RemoteControlHostCommand::SetSystemPower {
                                    power: personal_rns::remote_control::RemoteControlSystemPower::Awake,
                                }
                            );
                        }
                        hopspot::UiAction::BlankDisplay => {
                            let notice = hopspot::UiNotice::DisplayOff;
                            ui_state.show_notice(notice);
                            notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                            let _result = execute_hopspot_command!(
                                snapshots,
                                battery_state,
                                personal_rns::runtime::RemoteControlHostCommand::SetDisplayVisibility {
                                    visibility: personal_rns::remote_control::RemoteControlDisplayVisibility::Hidden,
                                }
                            );
                        }
                        hopspot::UiAction::ToggleDisplayAutoOff => {
                            if let Ok(current) = display.auto_off() {
                                let auto_off = match current {
                                    hopspot::display::DisplayAutoOff::Enabled => {
                                        personal_rns::remote_control::RemoteControlDisplayAutoOff::Disabled
                                    }
                                    hopspot::display::DisplayAutoOff::Disabled => {
                                        personal_rns::remote_control::RemoteControlDisplayAutoOff::Enabled
                                    }
                                };
                                let _result = execute_hopspot_command!(
                                    snapshots,
                                    battery_state,
                                    personal_rns::runtime::RemoteControlHostCommand::SetDisplayAutoOff { auto_off }
                                );
                                let notice = match auto_off {
                                    personal_rns::remote_control::RemoteControlDisplayAutoOff::Enabled => {
                                        hopspot::UiNotice::DisplayAutoOffOn
                                    }
                                    personal_rns::remote_control::RemoteControlDisplayAutoOff::Disabled => {
                                        hopspot::UiNotice::DisplayAutoOffOff
                                    }
                                };
                                ui_state.show_notice(notice);
                                notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                            }
                        }
                        #[cfg(feature = "board-t096")]
                        hopspot::UiAction::ControlGnss(command) => {
                            let power = match command {
                                hopspot::GnssReceiverCommand::Enable => {
                                    personal_rns::remote_control::RemoteControlGnssPower::On
                                }
                                hopspot::GnssReceiverCommand::Disable => {
                                    personal_rns::remote_control::RemoteControlGnssPower::Off
                                }
                            };
                            let _result = execute_hopspot_command!(
                                snapshots,
                                battery_state,
                                personal_rns::runtime::RemoteControlHostCommand::SetGnssPower {
                                    power
                                }
                            );
                        }
                        #[cfg(feature = "board-t114")]
                        hopspot::UiAction::ControlGnss(_) => {}
                        hopspot::UiAction::ToggleSelectedInterface => {
                            if let Some(card) = ui_state.selected_card(content.cards) {
                                let enabled = card.connection() != ConnectionState::Disabled;
                                let notice = if enabled {
                                    hopspot::UiNotice::TurningOff
                                } else {
                                    hopspot::UiNotice::TurningOn
                                };
                                ui_state.show_notice(notice);
                                notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                                let power = if enabled {
                                    personal_rns::remote_control::RemoteControlInterfacePower::Off
                                } else {
                                    personal_rns::remote_control::RemoteControlInterfacePower::On
                                };
                                let _result = execute_hopspot_command!(
                                    snapshots,
                                    battery_state,
                                    personal_rns::runtime::RemoteControlHostCommand::SetInterfacePower {
                                        id: card.id(),
                                        power,
                                    }
                                );
                            }
                        }
                        hopspot::UiAction::OpenDiscoveryGroupsEditor(id) => {
                            if id == BLE_SUPERVISOR_ID {
                                let groups =
                                    BluetoothAutoStatus::new(&BLE_SHARED).discovery_groups();
                                ui_state.open_discovery_groups_editor(id, &groups);
                            }
                        }
                        hopspot::UiAction::ReplaceDiscoveryGroups => {
                            let Some(replacement) = ui_state.take_discovery_group_replacement()
                            else {
                                continue;
                            };
                            let id = replacement.interface_id();
                            let groups = replacement.into_groups();
                            let result = execute_hopspot_command!(
                                snapshots,
                                battery_state,
                                personal_rns::runtime::RemoteControlHostCommand::ReplaceInterfaceDiscoveryGroups {
                                    id,
                                    groups: personal_rns::remote_control::RemoteControlDiscoveryGroups::new(groups),
                                }
                            );
                            let notice = match result {
                                Ok(personal_rns::runtime::RemoteControlHostResponse::ReplaceInterfaceDiscoveryGroups(
                                    personal_rns::remote_control::RemoteControlDiscoveryGroupsReplaceOutcome::Applied
                                    | personal_rns::remote_control::RemoteControlDiscoveryGroupsReplaceOutcome::Unchanged,
                                )) => hopspot::UiNotice::Saved,
                                Err(personal_rns::runtime::RemoteControlHostCommandError::Busy) => {
                                    hopspot::UiNotice::GroupsBusy
                                }
                                Err(personal_rns::runtime::RemoteControlHostCommandError::PersistenceFailed) => {
                                    hopspot::UiNotice::GroupsNotSaved
                                }
                                Err(personal_rns::runtime::RemoteControlHostCommandError::RollbackFailed) => {
                                    hopspot::UiNotice::GroupsRollbackFailed
                                }
                                _ => hopspot::UiNotice::GroupsApplyFailed,
                            };
                            ui_state.show_notice(notice);
                            notice_until_ms = Some((now_ms + NOTICE_MS, notice));
                        }
                        hopspot::UiAction::OpenSubGEditor => {
                            ui_state.open_subg_editor(working_subg_configuration);
                        }
                        action @ (hopspot::UiAction::SetSubGConfiguration(_)
                        | hopspot::UiAction::ClearSubGConfiguration) => {
                            let requested =
                                if let hopspot::UiAction::SetSubGConfiguration(configuration) =
                                    action
                                {
                                    SubGConfigurationState::Configured(configuration)
                                } else {
                                    SubGConfigurationState::Unconfigured
                                };
                            let result = match super::remote_control::apply_subg_configuration(
                                &mut lora_controller,
                                &mut subg_configuration_store,
                                &mut working_subg_configuration,
                                requested,
                            )
                            .await
                            {
                                Ok(()) => hopspot::SubGConfigurationChangeResult::Saved,
                                Err(personal_rns::runtime::RemoteControlHostCommandError::ApplyFailed) => {
                                    hopspot::SubGConfigurationChangeResult::ApplyFailed
                                }
                                Err(personal_rns::runtime::RemoteControlHostCommandError::RollbackFailed) => {
                                    hopspot::SubGConfigurationChangeResult::RollbackFailed
                                }
                                Err(personal_rns::runtime::RemoteControlHostCommandError::PersistenceFailed) => {
                                    hopspot::SubGConfigurationChangeResult::PersistenceFailed
                                }
                                Err(_) => hopspot::SubGConfigurationChangeResult::PersistenceUncertain,
                            };
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
                Either4::Second(()) | Either4::Third(()) => {}
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotBuildError {
    CapacityExhausted,
}

fn snapshots(
    lora: &EmbassyInterfaceStatus,
    usb: &EmbassyInterfaceStatus,
) -> Result<heapless::Vec<InterfaceSnapshot, { MEMBERS + 4 }>, SnapshotBuildError> {
    let ble = BluetoothAutoStatus::new(&BLE_SHARED);
    let mut entries: heapless::Vec<(&dyn InterfaceStatus, Membership), { MEMBERS + 4 }> =
        heapless::Vec::new();
    entries
        .push((lora, Membership::Independent))
        .map_err(|_| SnapshotBuildError::CapacityExhausted)?;
    entries
        .push((usb, Membership::Independent))
        .map_err(|_| SnapshotBuildError::CapacityExhausted)?;
    let supervisor_id = ble.id();
    entries
        .push((&ble, Membership::Independent))
        .map_err(|_| SnapshotBuildError::CapacityExhausted)?;
    for member in ble.members() {
        entries
            .push((member, Membership::FleetMember { supervisor_id }))
            .map_err(|_| SnapshotBuildError::CapacityExhausted)?;
    }
    let mut snapshots = heapless::Vec::new();
    for (status, membership) in &entries {
        let counts = INTERFACE_STORE.counts(status.id());
        snapshots
            .push(InterfaceSnapshot {
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
                radio: status.radio(),
                details: status.details(),
            })
            .map_err(|_| SnapshotBuildError::CapacityExhausted)?;
    }
    Ok(snapshots)
}

fn cards(
    snapshots: &[InterfaceSnapshot],
    subg_configuration: SubGConfigurationState,
    lora_id: InterfaceId,
    usb_id: InterfaceId,
) -> heapless::Vec<hopspot::Card, { MEMBERS + 4 }> {
    hopspot::snapshots_to_cards(snapshots, |id| {
        if id == lora_id {
            Some(hopspot::subg_card(subg_configuration))
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
