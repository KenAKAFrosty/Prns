use super::*;
use crate::persistence::S3SharedFlash;
use personal_hopspot_core::display::{DisplayBlankReason, DisplayVisibility, MonotonicMillis};
use personal_rns::remote_control::{
    RemoteControlApplyOutcome, RemoteControlCapabilities, RemoteControlDisplayAutoOff,
    RemoteControlDisplayVisibility, RemoteControlEspRadioMode, RemoteControlGnssPower,
    RemoteControlInterfacePower, RemoteControlLoRaOutcome, RemoteControlPowerOutcome,
    RemoteControlRequestKind, RemoteControlRequestSet, RemoteControlStationUplink,
    RemoteControlSystemPower, RemoteControlTargetSealingKey,
    RemoteControlWifiConfirmationRemaining, RemoteControlWifiCredentialRevision,
    RemoteControlWifiStageOutcome, RemoteControlWifiTransactionStatus,
    REMOTE_CONTROL_WIFI_CONFIRMATION_WINDOW_SECONDS,
};
use personal_rns::runtime::{
    RemoteControlHostCommand, RemoteControlHostCommandError, RemoteControlHostResponse,
};

pub(super) const REMOTE_CONTROL_COMMAND_DEPTH: usize = 1;
const RESPONSE_GRACE_PERIOD: Duration = Duration::from_millis(250);

fn display_now() -> MonotonicMillis {
    MonotonicMillis::new(embassy_time::Instant::now().as_millis())
}

pub(super) struct WifiConfirmation {
    controller: personal_rns::identity::IdentityHash,
    revision: RemoteControlWifiCredentialRevision,
    deadline: embassy_time::Instant,
}

impl WifiConfirmation {
    fn remaining(&self, now: embassy_time::Instant) -> u8 {
        let seconds = self.deadline.saturating_duration_since(now).as_secs();
        seconds.min(u64::from(REMOTE_CONTROL_WIFI_CONFIRMATION_WINDOW_SECONDS)) as u8
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScheduledAction {
    DisableInterface(InterfaceId),
    SleepSystem,
    SetRadioMode(RadioMode),
}

pub(super) struct ScheduledEffect {
    action: ScheduledAction,
    apply_at: embassy_time::Instant,
}

pub(super) struct S3RemoteControlContext<'a, D: S3DisplayRuntime> {
    pub snapshots: &'a [InterfaceSnapshot],
    pub usb_status: &'a EmbassyInterfaceStatus,
    pub lora_status: Option<&'a EmbassyInterfaceStatus>,
    pub wifi_status: Option<&'a AutoWifiStatus<MEMBERS>>,
    pub espnow_status: Option<&'a EmbassyInterfaceStatus>,
    pub tcp_status: Option<&'a EmbassyInterfaceStatus>,
    pub wifi_config: &'a mut HopspotWifiConfig,
    pub power: screen::PowerSnapshot,
    pub display: &'a mut D,
    pub radio_mode: RadioMode,
    pub system_awake: &'a mut bool,
    pub gnss_wanted: &'a mut bool,
    pub scheduled_effect: &'a mut Option<ScheduledEffect>,
    pub wifi_store: &'a mut screen::WifiConfigurationStore<S3SharedFlash>,
    pub wifi_key: &'a RemoteControlTargetSealingKey,
    pub wifi_confirmation: &'a mut Option<WifiConfirmation>,
    #[cfg(feature = "lora")]
    pub lora_controller: &'a mut personal_rns::lora::LoRaController<'static>,
    #[cfg(feature = "lora")]
    pub subg_store: &'a mut screen::SubGConfigurationStore<S3SharedFlash>,
    #[cfg(feature = "lora")]
    pub subg_configuration: &'a mut SubGConfigurationState,
}

pub(super) fn capabilities<B: Esp32S3Board>() -> RemoteControlCapabilities {
    let mut requests = RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
    for kind in [
        RemoteControlRequestKind::InventoryInterfaces,
        RemoteControlRequestKind::SetInterfacePower,
        RemoteControlRequestKind::InventoryInterfacePeers,
        RemoteControlRequestKind::InventoryInterfaceConfig,
        RemoteControlRequestKind::DescribeBuild,
        RemoteControlRequestKind::DescribePower,
        RemoteControlRequestKind::SetSystemPower,
        RemoteControlRequestKind::SetStationUplink,
        RemoteControlRequestKind::SetEspRadioMode,
        RemoteControlRequestKind::StageWifiCredentials,
        RemoteControlRequestKind::ActivateWifiCredentials,
        RemoteControlRequestKind::ConfirmWifiCredentials,
        RemoteControlRequestKind::CancelWifiCredentials,
        RemoteControlRequestKind::InspectWifiTransaction,
        RemoteControlRequestKind::InventoryControllers,
        RemoteControlRequestKind::AuthorizeController,
        RemoteControlRequestKind::RevokeController,
    ] {
        assert!(requests.insert(kind), "Remote Control capability is unique");
    }
    #[cfg(feature = "lora")]
    assert!(
        requests.insert(RemoteControlRequestKind::SetInterfaceLoRaProfile),
        "Remote Control capability is unique"
    );
    if B::Gnss::AVAILABILITY == screen::GnssAvailability::Available {
        assert!(
            requests.insert(RemoteControlRequestKind::SetGnssPower),
            "Remote Control capability is unique"
        );
    }
    if B::Display::REMOTE_VISIBILITY_CONTROL {
        assert!(
            requests.insert(RemoteControlRequestKind::SetDisplayVisibility),
            "Remote Control capability is unique"
        );
    }
    if B::Display::REMOTE_AUTO_OFF_CONTROL {
        assert!(
            requests.insert(RemoteControlRequestKind::SetDisplayAutoOff),
            "Remote Control capability is unique"
        );
    }
    RemoteControlCapabilities::from_requests(requests)
        .expect("the S3 capability set includes Describe")
}

pub(super) async fn execute<B: Esp32S3Board>(
    context: S3RemoteControlContext<'_, <B::Display as S3BoardDisplay>::Runtime>,
    command: RemoteControlHostCommand,
) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError> {
    match command {
        RemoteControlHostCommand::InventoryInterfaces { page } => {
            Ok(RemoteControlHostResponse::InventoryInterfaces(
                screen::remote_control_inventory_from_snapshots(context.snapshots, page),
            ))
        }
        RemoteControlHostCommand::SetInterfacePower { id, power } => {
            let Some(enabled) = interface_enabled(&context, id) else {
                return Ok(RemoteControlHostResponse::SetInterfacePower(
                    RemoteControlPowerOutcome::UnknownInterface,
                ));
            };
            let desired = power == RemoteControlInterfacePower::On;
            let outcome = if enabled == desired {
                RemoteControlPowerOutcome::Unchanged
            } else if desired {
                apply_interface_enabled(&context, id, true);
                RemoteControlPowerOutcome::Applied
            } else {
                schedule_effect(
                    context.scheduled_effect,
                    ScheduledAction::DisableInterface(id),
                )?;
                RemoteControlPowerOutcome::Scheduled
            };
            Ok(RemoteControlHostResponse::SetInterfacePower(outcome))
        }
        RemoteControlHostCommand::InventoryInterfacePeers { id, page } => {
            Ok(RemoteControlHostResponse::InventoryInterfacePeers(
                screen::remote_control_interface_peers_from_snapshots(context.snapshots, id, page),
            ))
        }
        RemoteControlHostCommand::InventoryInterfaceConfig { id } => {
            #[cfg(feature = "lora")]
            let lora_profile = match *context.subg_configuration {
                SubGConfigurationState::Configured(configuration) => {
                    let personal_rns::interfaces::subghz::ResolvedSubGMode::LoRa(profile) =
                        configuration.resolve();
                    Some(profile)
                }
                SubGConfigurationState::Unconfigured => None,
            };
            #[cfg(not(feature = "lora"))]
            let lora_profile = None;
            let outcome = screen::remote_control_interface_config_from_snapshots(
                context.snapshots,
                id,
                |snapshot, card| {
                    screen::decorate_hopspot_remote_control_card(
                        snapshot,
                        card,
                        None,
                        lora_profile,
                        context
                            .wifi_config
                            .has_station()
                            .then_some(context.wifi_config.ssid.as_str()),
                        None,
                    );
                },
            );
            Ok(RemoteControlHostResponse::InventoryInterfaceConfig(outcome))
        }
        RemoteControlHostCommand::DescribeBuild => Ok(RemoteControlHostResponse::DescribeBuild(
            screen::hopspot_remote_control_build_version(),
        )),
        RemoteControlHostCommand::DescribePower => {
            Ok(RemoteControlHostResponse::DescribePower(context.power))
        }
        RemoteControlHostCommand::SetSystemPower { power } => {
            let desired_awake = power == RemoteControlSystemPower::Awake;
            let outcome = if *context.system_awake == desired_awake {
                RemoteControlApplyOutcome::Unchanged
            } else if desired_awake {
                context
                    .display
                    .request_visible(display_now(), display_now)
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
                wake_system::<B>(&context);
                *context.system_awake = true;
                RemoteControlApplyOutcome::Applied
            } else {
                schedule_effect(context.scheduled_effect, ScheduledAction::SleepSystem)?;
                RemoteControlApplyOutcome::Scheduled
            };
            Ok(RemoteControlHostResponse::SetSystemPower(outcome))
        }
        RemoteControlHostCommand::SetGnssPower { power } => {
            if B::Gnss::snapshot().is_none() {
                return Err(RemoteControlHostCommandError::Unsupported);
            }
            let desired_on = power == RemoteControlGnssPower::On;
            let outcome = if *context.gnss_wanted == desired_on {
                RemoteControlApplyOutcome::Unchanged
            } else {
                *context.gnss_wanted = desired_on;
                if *context.system_awake {
                    B::Gnss::control(if desired_on {
                        screen::GnssReceiverCommand::Enable
                    } else {
                        screen::GnssReceiverCommand::Disable
                    });
                }
                RemoteControlApplyOutcome::Applied
            };
            Ok(RemoteControlHostResponse::SetGnssPower(outcome))
        }
        RemoteControlHostCommand::SetDisplayVisibility { visibility } => {
            let desired_visible = visibility == RemoteControlDisplayVisibility::Visible;
            let current = context.display.visibility();
            if current == DisplayVisibility::Unavailable {
                return Err(RemoteControlHostCommandError::Unsupported);
            }
            let currently_visible = current == DisplayVisibility::Visible;
            let outcome = if currently_visible == desired_visible {
                RemoteControlApplyOutcome::Unchanged
            } else {
                let now = display_now();
                let result = if desired_visible {
                    context.display.request_visible(now, display_now)
                } else {
                    context
                        .display
                        .schedule_blanking(now, DisplayBlankReason::DisplayOnly)
                        .and_then(|()| context.display.poll_blanking(now, display_now))
                };
                result.map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
                RemoteControlApplyOutcome::Applied
            };
            Ok(RemoteControlHostResponse::SetDisplayVisibility(outcome))
        }
        RemoteControlHostCommand::SetDisplayAutoOff { auto_off } => {
            let desired = match auto_off {
                RemoteControlDisplayAutoOff::Enabled => screen::display::DisplayAutoOff::Enabled,
                RemoteControlDisplayAutoOff::Disabled => screen::display::DisplayAutoOff::Disabled,
            };
            let current = context
                .display
                .auto_off()
                .map_err(|_| RemoteControlHostCommandError::Unsupported)?;
            let outcome = if current == desired {
                RemoteControlApplyOutcome::Unchanged
            } else {
                context
                    .display
                    .set_auto_off(desired, display_now())
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
                RemoteControlApplyOutcome::Applied
            };
            Ok(RemoteControlHostResponse::SetDisplayAutoOff(outcome))
        }
        RemoteControlHostCommand::SetStationUplink { id, uplink } => {
            let Some(status) = context.wifi_status.filter(|status| status.id() == id) else {
                return Err(RemoteControlHostCommandError::Unsupported);
            };
            let desired = uplink == RemoteControlStationUplink::Enabled;
            let outcome = if status.is_station_uplink_enabled() == desired {
                RemoteControlApplyOutcome::Unchanged
            } else {
                if desired {
                    status.enable_station_uplink();
                } else {
                    status.disable_station_uplink();
                }
                RemoteControlApplyOutcome::Applied
            };
            Ok(RemoteControlHostResponse::SetStationUplink(outcome))
        }
        RemoteControlHostCommand::SetEspRadioMode { mode } => {
            let desired = match mode {
                RemoteControlEspRadioMode::Bluetooth => RadioMode::Ble,
                RemoteControlEspRadioMode::AccessPoint => RadioMode::AccessPoint,
            };
            let outcome = if context.radio_mode == desired {
                RemoteControlApplyOutcome::Unchanged
            } else {
                schedule_effect(
                    context.scheduled_effect,
                    ScheduledAction::SetRadioMode(desired),
                )?;
                RemoteControlApplyOutcome::Scheduled
            };
            Ok(RemoteControlHostResponse::SetEspRadioMode(outcome))
        }
        #[cfg(feature = "lora")]
        RemoteControlHostCommand::SetInterfaceLoRaProfile { id, profile } => {
            let Some(status) = context.lora_status.filter(|status| status.id() == id) else {
                return Ok(RemoteControlHostResponse::SetInterfaceLoRaProfile(
                    RemoteControlLoRaOutcome::UnknownInterface,
                ));
            };
            let Some(profile) = profile.profile() else {
                return Err(RemoteControlHostCommandError::ApplyFailed);
            };
            let requested = SubGConfigurationState::Configured(
                personal_rns::interfaces::subghz::SubGConfiguration::manual_lora(profile),
            );
            apply_subg_configuration(
                context.lora_controller,
                context.subg_store,
                context.subg_configuration,
                requested,
            )
            .await?;
            status.enable();
            Ok(RemoteControlHostResponse::SetInterfaceLoRaProfile(
                RemoteControlLoRaOutcome::Applied,
            ))
        }
        RemoteControlHostCommand::StageWifiCredentials {
            controller,
            station,
        } => {
            let mut entropy = runtime_entropy();
            match context
                .wifi_store
                .stage(controller, station, context.wifi_key, &mut entropy)
                .await
            {
                screen::WifiConfigurationCommitOutcome::Committed(revision) => {
                    Ok(RemoteControlHostResponse::StageWifiCredentials(
                        RemoteControlWifiStageOutcome::Staged(revision),
                    ))
                }
                screen::WifiConfigurationCommitOutcome::NotCommitted(
                    screen::WifiConfigurationStoreError::Busy,
                ) => Err(RemoteControlHostCommandError::Busy),
                screen::WifiConfigurationCommitOutcome::NotCommitted(_)
                | screen::WifiConfigurationCommitOutcome::Indeterminate(_) => {
                    Err(RemoteControlHostCommandError::PersistenceFailed)
                }
            }
        }
        RemoteControlHostCommand::ActivateWifiCredentials {
            controller,
            revision,
        } => {
            let mut entropy = runtime_entropy();
            match context
                .wifi_store
                .activate(controller, revision, context.wifi_key, &mut entropy)
                .await
            {
                screen::WifiConfigurationCommitOutcome::Committed(station) => {
                    let ssid = station.ssid().to_string();
                    WIFI_CREDENTIALS.replace(screen::HopspotWifiCredentialUpdate {
                        revision: Some(revision),
                        station,
                    });
                    context.wifi_config.ssid = ssid;
                    if let Some(status) = context.wifi_status {
                        status.enable_station_uplink();
                    }
                    *context.wifi_confirmation = Some(WifiConfirmation {
                        controller,
                        revision,
                        deadline: embassy_time::Instant::now()
                            + Duration::from_secs(u64::from(
                                REMOTE_CONTROL_WIFI_CONFIRMATION_WINDOW_SECONDS,
                            )),
                    });
                    Ok(RemoteControlHostResponse::ActivateWifiCredentials(
                        RemoteControlApplyOutcome::Applied,
                    ))
                }
                screen::WifiConfigurationCommitOutcome::NotCommitted(
                    screen::WifiConfigurationStoreError::ControllerMismatch
                    | screen::WifiConfigurationStoreError::RevisionMismatch
                    | screen::WifiConfigurationStoreError::NoTransaction,
                ) => Err(RemoteControlHostCommandError::ApplyFailed),
                screen::WifiConfigurationCommitOutcome::NotCommitted(_)
                | screen::WifiConfigurationCommitOutcome::Indeterminate(_) => {
                    Err(RemoteControlHostCommandError::PersistenceFailed)
                }
            }
        }
        RemoteControlHostCommand::ConfirmWifiCredentials {
            controller,
            revision,
        } => {
            let Some(pending) = context.wifi_confirmation.as_ref() else {
                return Err(RemoteControlHostCommandError::ApplyFailed);
            };
            if pending.controller != controller || pending.revision != revision {
                return Err(RemoteControlHostCommandError::ApplyFailed);
            }
            if embassy_time::Instant::now() > pending.deadline {
                return Err(RemoteControlHostCommandError::ApplyFailed);
            }
            if WIFI_NETWORK_READY_REVISION.load(Ordering::Acquire) != revision.get() {
                return Err(RemoteControlHostCommandError::Busy);
            }
            let mut entropy = runtime_entropy();
            match context
                .wifi_store
                .confirm(controller, revision, context.wifi_key, &mut entropy)
                .await
            {
                screen::WifiConfigurationCommitOutcome::Committed(()) => {
                    *context.wifi_confirmation = None;
                    Ok(RemoteControlHostResponse::ConfirmWifiCredentials(
                        RemoteControlApplyOutcome::Applied,
                    ))
                }
                screen::WifiConfigurationCommitOutcome::NotCommitted(_)
                | screen::WifiConfigurationCommitOutcome::Indeterminate(_) => {
                    Err(RemoteControlHostCommandError::PersistenceFailed)
                }
            }
        }
        RemoteControlHostCommand::CancelWifiCredentials {
            controller,
            revision,
        } => {
            match rollback(context.wifi_store, context.wifi_key, controller, revision).await? {
                Some(ssid) => context.wifi_config.ssid = ssid,
                None => context.wifi_config.ssid.clear(),
            }
            *context.wifi_confirmation = None;
            Ok(RemoteControlHostResponse::CancelWifiCredentials(
                RemoteControlApplyOutcome::Applied,
            ))
        }
        RemoteControlHostCommand::InspectWifiTransaction { controller } => {
            let status = context
                .wifi_store
                .status(controller, context.wifi_key)
                .await
                .map_err(|error| match error {
                    screen::WifiConfigurationStoreError::ControllerMismatch => {
                        RemoteControlHostCommandError::Unsupported
                    }
                    _ => RemoteControlHostCommandError::PersistenceFailed,
                })?;
            let status = match status {
                screen::WifiConfigurationStatus::FactoryProvisioning => {
                    RemoteControlWifiTransactionStatus::FactoryProvisioning
                }
                screen::WifiConfigurationStatus::Confirmed { revision } => {
                    RemoteControlWifiTransactionStatus::Confirmed { revision }
                }
                screen::WifiConfigurationStatus::Transaction {
                    revision,
                    phase: screen::WifiConfigurationTransactionPhase::Staged,
                    ..
                } => RemoteControlWifiTransactionStatus::Staged { revision },
                screen::WifiConfigurationStatus::Transaction {
                    revision,
                    phase: screen::WifiConfigurationTransactionPhase::AwaitingConfirmation,
                    ..
                } => {
                    let remaining = context
                        .wifi_confirmation
                        .as_ref()
                        .filter(|pending| pending.revision == revision)
                        .map_or(0, |pending| pending.remaining(embassy_time::Instant::now()));
                    RemoteControlWifiTransactionStatus::AwaitingConfirmation {
                        revision,
                        remaining: RemoteControlWifiConfirmationRemaining::new(remaining)
                            .expect("the remaining confirmation window is bounded"),
                    }
                }
            };
            Ok(RemoteControlHostResponse::InspectWifiTransaction(status))
        }
        _ => Err(RemoteControlHostCommandError::Unsupported),
    }
}

fn schedule_effect(
    scheduled: &mut Option<ScheduledEffect>,
    action: ScheduledAction,
) -> Result<(), RemoteControlHostCommandError> {
    if let Some(existing) = scheduled {
        return if existing.action == action {
            Ok(())
        } else {
            Err(RemoteControlHostCommandError::Busy)
        };
    }
    *scheduled = Some(ScheduledEffect {
        action,
        apply_at: embassy_time::Instant::now() + RESPONSE_GRACE_PERIOD,
    });
    Ok(())
}

fn interface_enabled<D: S3DisplayRuntime>(
    context: &S3RemoteControlContext<'_, D>,
    id: InterfaceId,
) -> Option<bool> {
    if context.usb_status.id() == id {
        return Some(context.usb_status.is_enabled());
    }
    if let Some(status) = context.lora_status.filter(|status| status.id() == id) {
        return Some(status.is_enabled());
    }
    if let Some(status) = context.wifi_status.filter(|status| status.id() == id) {
        return Some(status.is_enabled());
    }
    if let Some(status) = context.espnow_status.filter(|status| status.id() == id) {
        return Some(status.is_enabled());
    }
    if let Some(status) = context.tcp_status.filter(|status| status.id() == id) {
        return Some(status.is_enabled());
    }
    if id == BLE_SUPERVISOR_ID {
        return Some(BluetoothAutoStatus::new(&BLE_SHARED).is_enabled());
    }
    None
}

fn apply_interface_enabled<D: S3DisplayRuntime>(
    context: &S3RemoteControlContext<'_, D>,
    id: InterfaceId,
    enabled: bool,
) {
    apply_interface_enabled_raw(
        context.usb_status,
        context.lora_status,
        context.wifi_status,
        context.espnow_status,
        context.tcp_status,
        id,
        enabled,
    );
}

fn apply_interface_enabled_raw(
    usb_status: &EmbassyInterfaceStatus,
    lora_status: Option<&EmbassyInterfaceStatus>,
    wifi_status: Option<&AutoWifiStatus<MEMBERS>>,
    espnow_status: Option<&EmbassyInterfaceStatus>,
    tcp_status: Option<&EmbassyInterfaceStatus>,
    id: InterfaceId,
    enabled: bool,
) {
    let set = |status: &EmbassyInterfaceStatus| {
        if enabled {
            status.enable();
        } else {
            status.disable();
        }
    };
    if usb_status.id() == id {
        set(usb_status);
    } else if let Some(status) = lora_status.filter(|status| status.id() == id) {
        set(status);
    } else if let Some(status) = wifi_status.filter(|status| status.id() == id) {
        if enabled {
            status.enable();
        } else {
            status.disable();
        }
    } else if let Some(status) = espnow_status.filter(|status| status.id() == id) {
        set(status);
    } else if let Some(status) = tcp_status.filter(|status| status.id() == id) {
        set(status);
    } else if id == BLE_SUPERVISOR_ID {
        let status = BluetoothAutoStatus::new(&BLE_SHARED);
        if enabled {
            status.enable();
        } else {
            status.disable();
        }
    }
}

fn wake_system<B: Esp32S3Board>(context: &S3RemoteControlContext<'_, impl S3DisplayRuntime>) {
    context.usb_status.enable();
    if let Some(status) = context.lora_status {
        status.enable();
    }
    if let Some(status) = context.wifi_status {
        status.enable_station_uplink();
        status.enable();
    }
    if let Some(status) = context.espnow_status {
        status.enable();
    }
    if let Some(status) = context.tcp_status {
        status.enable();
    }
    BluetoothAutoStatus::new(&BLE_SHARED).enable();
    if *context.gnss_wanted {
        B::Gnss::control(screen::GnssReceiverCommand::Enable);
    }
}

pub(super) fn apply_scheduled<B: Esp32S3Board>(
    scheduled: &mut Option<ScheduledEffect>,
    usb_status: &EmbassyInterfaceStatus,
    lora_status: Option<&EmbassyInterfaceStatus>,
    wifi_status: Option<&AutoWifiStatus<MEMBERS>>,
    espnow_status: Option<&EmbassyInterfaceStatus>,
    tcp_status: Option<&EmbassyInterfaceStatus>,
    display: &mut impl S3DisplayRuntime,
    system_awake: &mut bool,
) {
    let Some(effect) = scheduled.as_ref() else {
        return;
    };
    if embassy_time::Instant::now() < effect.apply_at {
        return;
    }
    let effect = scheduled.take().expect("the scheduled effect is present");
    match effect.action {
        ScheduledAction::DisableInterface(id) => apply_interface_enabled_raw(
            usb_status,
            lora_status,
            wifi_status,
            espnow_status,
            tcp_status,
            id,
            false,
        ),
        ScheduledAction::SleepSystem => {
            let now = display_now();
            if let Err(error) = display
                .schedule_blanking(now, DisplayBlankReason::SystemSleep)
                .and_then(|()| display.poll_blanking(now, display_now))
            {
                log::error!("scheduled system-sleep display apply failed: {error:?}");
            }
            usb_status.disable();
            if let Some(status) = lora_status {
                status.disable();
            }
            if let Some(status) = wifi_status {
                status.disable();
                status.disable_station_uplink();
            }
            if let Some(status) = espnow_status {
                status.disable();
            }
            if let Some(status) = tcp_status {
                status.disable();
            }
            BluetoothAutoStatus::new(&BLE_SHARED).disable();
            B::Gnss::control(screen::GnssReceiverCommand::Disable);
            *system_awake = false;
        }
        ScheduledAction::SetRadioMode(mode) => request_radio_mode(mode),
    }
}

#[cfg(feature = "lora")]
pub(super) async fn apply_subg_configuration(
    controller: &mut personal_rns::lora::LoRaController<'static>,
    store: &mut screen::SubGConfigurationStore<S3SharedFlash>,
    active: &mut SubGConfigurationState,
    requested: SubGConfigurationState,
) -> Result<(), RemoteControlHostCommandError> {
    let previous = *active;
    if controller.apply_configuration(requested).await == LoRaApplyOutcome::Rejected {
        return Err(RemoteControlHostCommandError::ApplyFailed);
    }
    *active = requested;
    let persistence = match requested {
        SubGConfigurationState::Configured(configuration) => store.save(configuration).await,
        SubGConfigurationState::Unconfigured => store.clear().await,
    };
    match persistence {
        screen::SubGConfigurationCommitOutcome::Committed => Ok(()),
        screen::SubGConfigurationCommitOutcome::Indeterminate(_) => {
            Err(RemoteControlHostCommandError::PersistenceFailed)
        }
        screen::SubGConfigurationCommitOutcome::NotCommitted(_) => {
            if controller.apply_configuration(previous).await == LoRaApplyOutcome::Applied {
                *active = previous;
                Err(RemoteControlHostCommandError::PersistenceFailed)
            } else {
                Err(RemoteControlHostCommandError::RollbackFailed)
            }
        }
    }
}

pub(super) async fn rollback_expired(
    wifi_store: &mut screen::WifiConfigurationStore<S3SharedFlash>,
    wifi_key: &RemoteControlTargetSealingKey,
    wifi_confirmation: &mut Option<WifiConfirmation>,
    wifi_config: &mut HopspotWifiConfig,
) {
    let Some(pending) = wifi_confirmation.as_ref() else {
        return;
    };
    if embassy_time::Instant::now() <= pending.deadline {
        return;
    }
    let controller = pending.controller;
    let revision = pending.revision;
    match rollback(wifi_store, wifi_key, controller, revision).await {
        Ok(ssid) => {
            match ssid {
                Some(ssid) => wifi_config.ssid = ssid,
                None => wifi_config.ssid.clear(),
            }
            *wifi_confirmation = None;
            log::warn!("wifi-config: confirmation timed out; restored confirmed credentials");
        }
        Err(error) => log::error!("wifi-config: timed-out rollback failed: {error:?}"),
    }
}

async fn rollback(
    wifi_store: &mut screen::WifiConfigurationStore<S3SharedFlash>,
    wifi_key: &RemoteControlTargetSealingKey,
    controller: personal_rns::identity::IdentityHash,
    revision: RemoteControlWifiCredentialRevision,
) -> Result<Option<String>, RemoteControlHostCommandError> {
    let mut entropy = runtime_entropy();
    match wifi_store
        .cancel(controller, revision, wifi_key, &mut entropy)
        .await
    {
        screen::WifiConfigurationCommitOutcome::Committed(Some(station)) => {
            let ssid = station.ssid().to_string();
            WIFI_CREDENTIALS.replace(screen::HopspotWifiCredentialUpdate {
                revision: None,
                station,
            });
            Ok(Some(ssid))
        }
        screen::WifiConfigurationCommitOutcome::Committed(None) => {
            let (factory_wifi, _) = hopspot_wifi_config();
            if factory_wifi.has_station() {
                let station = personal_rns::remote_control::RemoteControlWifiStation::parse(
                    &factory_wifi.ssid,
                    &factory_wifi.password,
                )
                .ok_or(RemoteControlHostCommandError::RollbackFailed)?;
                let ssid = station.ssid().to_string();
                WIFI_CREDENTIALS.replace(screen::HopspotWifiCredentialUpdate {
                    revision: None,
                    station,
                });
                Ok(Some(ssid))
            } else {
                WIFI_CREDENTIALS.clear();
                Ok(None)
            }
        }
        screen::WifiConfigurationCommitOutcome::NotCommitted(_)
        | screen::WifiConfigurationCommitOutcome::Indeterminate(_) => {
            Err(RemoteControlHostCommandError::RollbackFailed)
        }
    }
}
