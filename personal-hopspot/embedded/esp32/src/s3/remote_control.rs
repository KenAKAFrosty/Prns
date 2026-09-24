use super::*;
use crate::persistence::S3SharedFlash;
use personal_hopspot_core::display::{DisplayBlankReason, DisplayVisibility, MonotonicMillis};
use personal_rns::remote_control::{
    RemoteControlApplyOutcome, RemoteControlCapabilities, RemoteControlDiscoveryGroups,
    RemoteControlDiscoveryGroupsInventoryOutcome, RemoteControlDiscoveryGroupsReplaceOutcome,
    RemoteControlDisplayAutoOff, RemoteControlDisplayVisibility, RemoteControlEspRadioMode,
    RemoteControlGnssPower, RemoteControlGroupOutcome, RemoteControlInterfacePower,
    RemoteControlLoRaOutcome, RemoteControlPowerOutcome, RemoteControlRequestKind,
    RemoteControlStationUplink, RemoteControlSystemPower, RemoteControlTargetSealingKey,
    RemoteControlWifiConfirmationRemaining, RemoteControlWifiCredentialRevision,
    RemoteControlWifiStageOutcome, RemoteControlWifiTransactionStatus,
    REMOTE_CONTROL_WIFI_CONFIRMATION_WINDOW_SECONDS,
};
use personal_rns::runtime::{
    RemoteControlHostCommand, RemoteControlHostCommandError, RemoteControlHostResponse,
};

pub(super) const REMOTE_CONTROL_COMMAND_DEPTH: usize = 1;
const RESPONSE_GRACE_PERIOD: Duration = Duration::from_millis(250);
const SYSTEM_AWAKE: u16 = 1 << 0;
const USB_ENABLED: u16 = 1 << 1;
const LORA_ENABLED: u16 = 1 << 2;
const WIFI_ENABLED: u16 = 1 << 3;
const ESPNOW_ENABLED: u16 = 1 << 4;
const TCP_ENABLED: u16 = 1 << 5;
const BLUETOOTH_ENABLED: u16 = 1 << 6;
const STATION_UPLINK_ENABLED: u16 = 1 << 7;
const GNSS_ENABLED: u16 = 1 << 8;

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

pub(super) struct SystemIntent(u16);

impl SystemIntent {
    pub(super) fn from_status(
        usb_status: &EmbassyInterfaceStatus,
        lora_status: Option<&EmbassyInterfaceStatus>,
        wifi_status: Option<&AutoWifiStatus<MEMBERS>>,
        espnow_status: Option<&EmbassyInterfaceStatus>,
        tcp_status: Option<&EmbassyInterfaceStatus>,
        gnss_enabled: bool,
    ) -> Self {
        let mut bits = SYSTEM_AWAKE;
        if usb_status.is_enabled() {
            bits |= USB_ENABLED;
        }
        if lora_status.is_some_and(|status| status.is_enabled()) {
            bits |= LORA_ENABLED;
        }
        if let Some(status) = wifi_status {
            if status.is_enabled() {
                bits |= WIFI_ENABLED;
            }
            if status.is_station_uplink_enabled() {
                bits |= STATION_UPLINK_ENABLED;
            }
        }
        if espnow_status.is_some_and(|status| status.is_enabled()) {
            bits |= ESPNOW_ENABLED;
        }
        if tcp_status.is_some_and(|status| status.is_enabled()) {
            bits |= TCP_ENABLED;
        }
        if BluetoothAutoStatus::new(&BLE_SHARED).is_enabled() {
            bits |= BLUETOOTH_ENABLED;
        }
        if gnss_enabled {
            bits |= GNSS_ENABLED;
        }
        Self(bits)
    }

    fn is_awake(&self) -> bool {
        self.0 & SYSTEM_AWAKE != 0
    }

    fn set_awake(&mut self, awake: bool) {
        self.set(SYSTEM_AWAKE, awake);
    }

    fn gnss_enabled(&self) -> bool {
        self.0 & GNSS_ENABLED != 0
    }

    fn set_gnss_enabled(&mut self, enabled: bool) {
        self.set(GNSS_ENABLED, enabled);
    }

    fn set(&mut self, bit: u16, enabled: bool) {
        if enabled {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
    }
}

enum ScheduledAction {
    DisableInterface(InterfaceId),
    DisableStationUplink,
    ReconcileInterfaces,
    SleepSystem,
    SetRadioMode(RadioMode),
    ActivateWifi {
        controller: personal_rns::identity::IdentityHash,
        revision: RemoteControlWifiCredentialRevision,
        station: personal_rns::remote_control::RemoteControlWifiStation,
    },
    RestoreWifi {
        station: Option<personal_rns::remote_control::RemoteControlWifiStation>,
    },
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
    pub system: &'a mut SystemIntent,
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
    let mut capabilities = RemoteControlCapabilities::describe_only();
    for kind in [
        RemoteControlRequestKind::AnnounceSelf,
        RemoteControlRequestKind::InventoryInterfaces,
        RemoteControlRequestKind::SetInterfacePower,
        RemoteControlRequestKind::InventoryInterfacePeers,
        RemoteControlRequestKind::InventoryInterfaceConfig,
        RemoteControlRequestKind::SetInterfaceGroup,
        RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups,
        RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups,
        RemoteControlRequestKind::DescribeBuild,
        RemoteControlRequestKind::InventoryPathTable,
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
        capabilities = capabilities.with_request(kind);
    }
    #[cfg(feature = "lora")]
    {
        capabilities = capabilities.with_request(RemoteControlRequestKind::SetInterfaceLoRaProfile);
    }
    if B::Gnss::AVAILABILITY == screen::GnssAvailability::Available {
        capabilities = capabilities.with_request(RemoteControlRequestKind::SetGnssPower);
    }
    if B::Display::REMOTE_VISIBILITY_CONTROL {
        capabilities = capabilities.with_request(RemoteControlRequestKind::SetDisplayVisibility);
    }
    if B::Display::REMOTE_AUTO_OFF_CONTROL {
        capabilities = capabilities.with_request(RemoteControlRequestKind::SetDisplayAutoOff);
    }
    capabilities
}

pub(super) async fn execute<B: Esp32S3Board>(
    mut context: S3RemoteControlContext<'_, <B::Display as S3BoardDisplay>::Runtime>,
    command: RemoteControlHostCommand,
) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError> {
    match command {
        RemoteControlHostCommand::InventoryInterfaces { page } => {
            Ok(RemoteControlHostResponse::InventoryInterfaces(
                screen::remote_control_inventory_from_snapshots(context.snapshots, page)
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?,
            ))
        }
        RemoteControlHostCommand::SetInterfacePower { id, power } => {
            let Some(enabled) = desired_interface_enabled(&context, id) else {
                return Ok(RemoteControlHostResponse::SetInterfacePower(
                    RemoteControlPowerOutcome::UnknownInterface,
                ));
            };
            let desired = power == RemoteControlInterfacePower::On;
            let outcome = if desired && !context.system.is_awake() {
                return Err(RemoteControlHostCommandError::Busy);
            } else if !desired && !context.system.is_awake() {
                if enabled {
                    record_desired_interface(&mut context, id, false);
                    RemoteControlPowerOutcome::Applied
                } else {
                    RemoteControlPowerOutcome::Unchanged
                }
            } else if desired && has_pending_sleep(context.scheduled_effect) {
                return Err(RemoteControlHostCommandError::Busy);
            } else if !desired && has_pending_sleep(context.scheduled_effect) {
                record_desired_interface(&mut context, id, false);
                RemoteControlPowerOutcome::Scheduled
            } else if desired && cancel_pending_interface_disable(context.scheduled_effect, id) {
                set_desired_interface(&mut context, id, true);
                RemoteControlPowerOutcome::Unchanged
            } else if enabled == desired {
                RemoteControlPowerOutcome::Unchanged
            } else if desired {
                set_desired_interface(&mut context, id, true);
                RemoteControlPowerOutcome::Applied
            } else {
                schedule_effect(
                    context.scheduled_effect,
                    ScheduledAction::DisableInterface(id),
                )?;
                record_desired_interface(&mut context, id, false);
                RemoteControlPowerOutcome::Scheduled
            };
            Ok(RemoteControlHostResponse::SetInterfacePower(outcome))
        }
        RemoteControlHostCommand::SetInterfaceMode { .. } => {
            Err(RemoteControlHostCommandError::Unsupported)
        }
        RemoteControlHostCommand::SetInterfaceGroup { id, group } => {
            let groups = personal_rns::interfaces::DiscoveryGroupSet::from_singleton(group);
            let outcome = replace_discovery_groups(&context, id, groups).await?;
            let outcome = match outcome {
                RemoteControlDiscoveryGroupsReplaceOutcome::Applied
                | RemoteControlDiscoveryGroupsReplaceOutcome::Unchanged => {
                    RemoteControlGroupOutcome::Applied
                }
                RemoteControlDiscoveryGroupsReplaceOutcome::UnknownInterface => {
                    RemoteControlGroupOutcome::UnknownInterface
                }
                RemoteControlDiscoveryGroupsReplaceOutcome::Unsupported => {
                    return Err(RemoteControlHostCommandError::Unsupported);
                }
            };
            Ok(RemoteControlHostResponse::SetInterfaceGroup(outcome))
        }
        RemoteControlHostCommand::InventoryInterfaceDiscoveryGroups { id } => {
            let outcome = if id == BLE_SUPERVISOR_ID {
                RemoteControlDiscoveryGroupsInventoryOutcome::Groups(
                    RemoteControlDiscoveryGroups::new(
                        BluetoothAutoStatus::new(&BLE_SHARED).discovery_groups(),
                    ),
                )
            } else if let Some(status) = context.wifi_status.filter(|status| status.id() == id) {
                RemoteControlDiscoveryGroupsInventoryOutcome::Groups(
                    RemoteControlDiscoveryGroups::new(status.discovery_groups()),
                )
            } else {
                RemoteControlDiscoveryGroupsInventoryOutcome::UnknownInterface
            };
            Ok(RemoteControlHostResponse::InventoryInterfaceDiscoveryGroups(outcome))
        }
        RemoteControlHostCommand::ReplaceInterfaceDiscoveryGroups { id, groups } => {
            Ok(RemoteControlHostResponse::ReplaceInterfaceDiscoveryGroups(
                replace_discovery_groups(&context, id, groups.into_groups()).await?,
            ))
        }
        RemoteControlHostCommand::InventoryInterfacePeers { id, page } => {
            Ok(RemoteControlHostResponse::InventoryInterfacePeers(
                screen::remote_control_interface_peers_from_snapshots(context.snapshots, id, page)
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?,
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
            let discovery_groups = if id == BLE_SUPERVISOR_ID {
                Some(BluetoothAutoStatus::new(&BLE_SHARED).discovery_groups())
            } else {
                context
                    .wifi_status
                    .filter(|status| status.id() == id)
                    .map(|status| status.discovery_groups())
            };
            let discovery_group = discovery_groups
                .as_ref()
                .and_then(personal_hopspot_core::singleton_discovery_group);
            let outcome = screen::remote_control_interface_config_from_snapshots(
                context.snapshots,
                id,
                |snapshot, card| {
                    screen::decorate_hopspot_remote_control_card(
                        snapshot,
                        card,
                        discovery_group,
                        lora_profile,
                        context
                            .wifi_config
                            .has_station()
                            .then_some(context.wifi_config.ssid.as_str()),
                        None,
                    )
                },
            )
            .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
            Ok(RemoteControlHostResponse::InventoryInterfaceConfig(outcome))
        }
        RemoteControlHostCommand::DescribeBuild => Ok(RemoteControlHostResponse::DescribeBuild(
            screen::hopspot_remote_control_build_version()
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?,
        )),
        RemoteControlHostCommand::DescribePower => {
            Ok(RemoteControlHostResponse::DescribePower(context.power))
        }
        RemoteControlHostCommand::SetSystemPower { power } => {
            let desired_awake = power == RemoteControlSystemPower::Awake;
            let outcome = if desired_awake && cancel_pending_sleep(context.scheduled_effect) {
                if runtime_matches_desired(&context) {
                    RemoteControlApplyOutcome::Unchanged
                } else {
                    schedule_effect(
                        context.scheduled_effect,
                        ScheduledAction::ReconcileInterfaces,
                    )?;
                    RemoteControlApplyOutcome::Scheduled
                }
            } else if context.system.is_awake() == desired_awake {
                RemoteControlApplyOutcome::Unchanged
            } else if desired_awake {
                context
                    .display
                    .request_visible(display_now(), display_now)
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
                restore_desired_system::<B>(&context);
                context.system.set_awake(true);
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
            let outcome = if context.system.gnss_enabled() == desired_on {
                RemoteControlApplyOutcome::Unchanged
            } else {
                context.system.set_gnss_enabled(desired_on);
                if context.system.is_awake() {
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
            if desired_visible && !context.system.is_awake() {
                return Err(RemoteControlHostCommandError::Busy);
            }
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
            let enabled = context.system.0 & STATION_UPLINK_ENABLED != 0;
            let outcome = if desired && !context.system.is_awake() {
                return Err(RemoteControlHostCommandError::Busy);
            } else if !desired && !context.system.is_awake() {
                if enabled {
                    context.system.set(STATION_UPLINK_ENABLED, false);
                    RemoteControlApplyOutcome::Applied
                } else {
                    RemoteControlApplyOutcome::Unchanged
                }
            } else if desired && has_pending_sleep(context.scheduled_effect) {
                return Err(RemoteControlHostCommandError::Busy);
            } else if !desired && has_pending_sleep(context.scheduled_effect) {
                context.system.set(STATION_UPLINK_ENABLED, false);
                RemoteControlApplyOutcome::Scheduled
            } else if desired && cancel_pending_station_disable(context.scheduled_effect) {
                context.system.set(STATION_UPLINK_ENABLED, true);
                status.enable_station_uplink();
                RemoteControlApplyOutcome::Unchanged
            } else if enabled == desired {
                RemoteControlApplyOutcome::Unchanged
            } else if desired {
                context.system.set(STATION_UPLINK_ENABLED, true);
                status.enable_station_uplink();
                RemoteControlApplyOutcome::Applied
            } else {
                schedule_effect(
                    context.scheduled_effect,
                    ScheduledAction::DisableStationUplink,
                )?;
                context.system.set(STATION_UPLINK_ENABLED, false);
                RemoteControlApplyOutcome::Scheduled
            };
            Ok(RemoteControlHostResponse::SetStationUplink(outcome))
        }
        RemoteControlHostCommand::SetEspRadioMode { mode } => {
            let desired = match mode {
                RemoteControlEspRadioMode::Bluetooth => RadioMode::Ble,
                RemoteControlEspRadioMode::AccessPoint => RadioMode::AccessPoint,
            };
            let outcome = if cancel_pending_radio_mode(context.scheduled_effect, desired)
                || context.radio_mode == desired
            {
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
            let Some(_status) = context.lora_status.filter(|status| status.id() == id) else {
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
            Ok(RemoteControlHostResponse::SetInterfaceLoRaProfile(
                RemoteControlLoRaOutcome::Applied,
            ))
        }
        RemoteControlHostCommand::StageWifiCredentials {
            controller,
            station,
        } => {
            if context.wifi_confirmation.is_some()
                || context.scheduled_effect.as_ref().is_some_and(|effect| {
                    matches!(effect.action, ScheduledAction::ActivateWifi { .. })
                })
            {
                return Err(RemoteControlHostCommandError::Busy);
            }
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
                screen::WifiConfigurationCommitOutcome::NotCommitted(_) => {
                    Err(RemoteControlHostCommandError::PersistenceFailed)
                }
                screen::WifiConfigurationCommitOutcome::Indeterminate(_) => {
                    recover_wifi_store(context.wifi_store, context.wifi_key, context.wifi_config)
                        .await?;
                    Err(RemoteControlHostCommandError::PersistenceFailed)
                }
            }
        }
        RemoteControlHostCommand::ActivateWifiCredentials {
            controller,
            revision,
        } => {
            if let Some(pending) = context.wifi_confirmation.as_ref() {
                return if pending.controller == controller && pending.revision == revision {
                    Ok(RemoteControlHostResponse::ActivateWifiCredentials(
                        RemoteControlApplyOutcome::Unchanged,
                    ))
                } else {
                    Err(RemoteControlHostCommandError::Busy)
                };
            }
            if let Some(effect) = context.scheduled_effect.as_ref() {
                return if is_wifi_activation(&effect.action, controller, revision) {
                    Ok(RemoteControlHostResponse::ActivateWifiCredentials(
                        RemoteControlApplyOutcome::Scheduled,
                    ))
                } else {
                    Err(RemoteControlHostCommandError::Busy)
                };
            }
            let mut entropy = runtime_entropy();
            match context
                .wifi_store
                .activate(controller, revision, context.wifi_key, &mut entropy)
                .await
            {
                screen::WifiConfigurationCommitOutcome::Committed(station) => {
                    schedule_effect(
                        context.scheduled_effect,
                        ScheduledAction::ActivateWifi {
                            controller,
                            revision,
                            station,
                        },
                    )?;
                    Ok(RemoteControlHostResponse::ActivateWifiCredentials(
                        RemoteControlApplyOutcome::Scheduled,
                    ))
                }
                screen::WifiConfigurationCommitOutcome::NotCommitted(
                    screen::WifiConfigurationStoreError::ControllerMismatch
                    | screen::WifiConfigurationStoreError::RevisionMismatch
                    | screen::WifiConfigurationStoreError::NoTransaction,
                ) => Err(RemoteControlHostCommandError::ApplyFailed),
                screen::WifiConfigurationCommitOutcome::NotCommitted(_) => {
                    Err(RemoteControlHostCommandError::PersistenceFailed)
                }
                screen::WifiConfigurationCommitOutcome::Indeterminate(_) => {
                    recover_wifi_store(context.wifi_store, context.wifi_key, context.wifi_config)
                        .await?;
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
                screen::WifiConfigurationCommitOutcome::NotCommitted(_) => {
                    Err(RemoteControlHostCommandError::PersistenceFailed)
                }
                screen::WifiConfigurationCommitOutcome::Indeterminate(_) => {
                    let recovered = recover_wifi_store(
                        context.wifi_store,
                        context.wifi_key,
                        context.wifi_config,
                    )
                    .await?;
                    *context.wifi_confirmation = None;
                    if recovered == (screen::WifiConfigurationStatus::Confirmed { revision }) {
                        Ok(RemoteControlHostResponse::ConfirmWifiCredentials(
                            RemoteControlApplyOutcome::Applied,
                        ))
                    } else {
                        Err(RemoteControlHostCommandError::PersistenceFailed)
                    }
                }
            }
        }
        RemoteControlHostCommand::CancelWifiCredentials {
            controller,
            revision,
        } => {
            let activation_scheduled = context
                .scheduled_effect
                .as_ref()
                .is_some_and(|effect| is_wifi_activation(&effect.action, controller, revision));
            let confirmation_active = context.wifi_confirmation.as_ref().is_some_and(|pending| {
                pending.controller == controller && pending.revision == revision
            });
            if confirmation_active && context.scheduled_effect.is_some() {
                return Err(RemoteControlHostCommandError::Busy);
            }
            let rollback = persist_wifi_rollback(
                context.wifi_store,
                context.wifi_key,
                controller,
                revision,
                context.wifi_config,
            )
            .await?;
            let outcome = if confirmation_active {
                let station = match rollback {
                    WifiRollbackOutcome::Committed(active) => resolve_active_wifi(active)?,
                    WifiRollbackOutcome::Recovered => {
                        *context.wifi_confirmation = None;
                        return Ok(RemoteControlHostResponse::CancelWifiCredentials(
                            RemoteControlApplyOutcome::Applied,
                        ));
                    }
                };
                // A pending activation owns the one scheduled-effect slot. Replace it only after
                // the cancellation is durable, so a failed persistence transaction cannot alter
                // the running credentials.
                *context.scheduled_effect = None;
                schedule_effect(
                    context.scheduled_effect,
                    ScheduledAction::RestoreWifi { station },
                )?;
                RemoteControlApplyOutcome::Scheduled
            } else if activation_scheduled {
                // The candidate has not reached the station task yet, so cancelling the delayed
                // activation is itself the complete runtime effect.
                *context.scheduled_effect = None;
                RemoteControlApplyOutcome::Applied
            } else {
                RemoteControlApplyOutcome::Applied
            };
            *context.wifi_confirmation = None;
            Ok(RemoteControlHostResponse::CancelWifiCredentials(outcome))
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
                            .ok_or(RemoteControlHostCommandError::ApplyFailed)?,
                    }
                }
            };
            Ok(RemoteControlHostResponse::InspectWifiTransaction(status))
        }
        _ => Err(RemoteControlHostCommandError::Unsupported),
    }
}

async fn replace_discovery_groups<D: S3DisplayRuntime>(
    context: &S3RemoteControlContext<'_, D>,
    id: InterfaceId,
    desired: personal_rns::interfaces::DiscoveryGroupSet,
) -> Result<RemoteControlDiscoveryGroupsReplaceOutcome, RemoteControlHostCommandError> {
    #[derive(Clone, Copy)]
    enum Target<'a> {
        Bluetooth(BluetoothAutoStatus<BLE_PEER_CAPACITY>),
        Wifi(&'a AutoWifiStatus<MEMBERS>),
    }

    let target = if id == BLE_SUPERVISOR_ID {
        Target::Bluetooth(BluetoothAutoStatus::new(&BLE_SHARED))
    } else if let Some(status) = context.wifi_status.filter(|status| status.id() == id) {
        Target::Wifi(status)
    } else {
        return Ok(RemoteControlDiscoveryGroupsReplaceOutcome::UnknownInterface);
    };
    let previous = match target {
        Target::Bluetooth(status) => status.discovery_groups(),
        Target::Wifi(status) => status.discovery_groups(),
    };
    if previous == desired {
        return Ok(RemoteControlDiscoveryGroupsReplaceOutcome::Unchanged);
    }
    let prepared = screen::persist_discovery_group_replacement(id, &desired).await?;
    let applied = match target {
        Target::Bluetooth(status) => status.replace_discovery_groups(&desired).await,
        Target::Wifi(status) => status.replace_discovery_groups(desired).await,
    };
    if matches!(
        applied,
        personal_rns::interfaces::DiscoveryGroupApplyOutcome::Applied
            | personal_rns::interfaces::DiscoveryGroupApplyOutcome::Unchanged
    ) {
        return Ok(RemoteControlDiscoveryGroupsReplaceOutcome::Applied);
    }
    let durable_restored = screen::rollback_discovery_group_replacement(&prepared)
        .await
        .is_ok();
    let runtime_restored = match target {
        Target::Bluetooth(status) => {
            status.discovery_groups() == previous
                || matches!(
                    status.replace_discovery_groups(&previous).await,
                    personal_rns::interfaces::DiscoveryGroupApplyOutcome::Applied
                        | personal_rns::interfaces::DiscoveryGroupApplyOutcome::Unchanged
                )
        }
        Target::Wifi(status) => {
            status.discovery_groups() == previous
                || matches!(
                    status.replace_discovery_groups(previous).await,
                    personal_rns::interfaces::DiscoveryGroupApplyOutcome::Applied
                        | personal_rns::interfaces::DiscoveryGroupApplyOutcome::Unchanged
                )
        }
    };
    if !durable_restored || !runtime_restored {
        return Err(RemoteControlHostCommandError::RollbackFailed);
    }
    Err(RemoteControlHostCommandError::ApplyFailed)
}

fn cancel_pending_interface_disable(
    scheduled: &mut Option<ScheduledEffect>,
    id: InterfaceId,
) -> bool {
    if matches!(
        scheduled.as_ref().map(|effect| &effect.action),
        Some(ScheduledAction::DisableInterface(pending)) if *pending == id
    ) {
        *scheduled = None;
        true
    } else {
        false
    }
}

fn cancel_pending_station_disable(scheduled: &mut Option<ScheduledEffect>) -> bool {
    if matches!(
        scheduled.as_ref().map(|effect| &effect.action),
        Some(ScheduledAction::DisableStationUplink)
    ) {
        *scheduled = None;
        true
    } else {
        false
    }
}

fn cancel_pending_sleep(scheduled: &mut Option<ScheduledEffect>) -> bool {
    if matches!(
        scheduled.as_ref().map(|effect| &effect.action),
        Some(ScheduledAction::SleepSystem)
    ) {
        *scheduled = None;
        true
    } else {
        false
    }
}

fn has_pending_sleep(scheduled: &Option<ScheduledEffect>) -> bool {
    matches!(
        scheduled.as_ref().map(|effect| &effect.action),
        Some(ScheduledAction::SleepSystem)
    )
}

fn cancel_pending_radio_mode(scheduled: &mut Option<ScheduledEffect>, desired: RadioMode) -> bool {
    if matches!(
        scheduled.as_ref().map(|effect| &effect.action),
        Some(ScheduledAction::SetRadioMode(pending)) if *pending != desired
    ) {
        *scheduled = None;
        true
    } else {
        false
    }
}

fn schedule_effect(
    scheduled: &mut Option<ScheduledEffect>,
    action: ScheduledAction,
) -> Result<(), RemoteControlHostCommandError> {
    if let Some(existing) = scheduled {
        return if same_scheduled_action(&existing.action, &action) {
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

fn same_scheduled_action(first: &ScheduledAction, second: &ScheduledAction) -> bool {
    match (first, second) {
        (ScheduledAction::DisableInterface(first), ScheduledAction::DisableInterface(second)) => {
            first == second
        }
        (ScheduledAction::DisableStationUplink, ScheduledAction::DisableStationUplink)
        | (ScheduledAction::ReconcileInterfaces, ScheduledAction::ReconcileInterfaces) => true,
        (ScheduledAction::SleepSystem, ScheduledAction::SleepSystem) => true,
        (ScheduledAction::SetRadioMode(first), ScheduledAction::SetRadioMode(second)) => {
            first == second
        }
        (
            ScheduledAction::ActivateWifi {
                controller: first_controller,
                revision: first_revision,
                ..
            },
            ScheduledAction::ActivateWifi {
                controller: second_controller,
                revision: second_revision,
                ..
            },
        ) => first_controller == second_controller && first_revision == second_revision,
        _ => false,
    }
}

fn is_wifi_activation(
    action: &ScheduledAction,
    controller: personal_rns::identity::IdentityHash,
    revision: RemoteControlWifiCredentialRevision,
) -> bool {
    matches!(
        action,
        ScheduledAction::ActivateWifi {
            controller: scheduled_controller,
            revision: scheduled_revision,
            ..
        } if *scheduled_controller == controller && *scheduled_revision == revision
    )
}

fn interface_bit<D: S3DisplayRuntime>(
    context: &S3RemoteControlContext<'_, D>,
    id: InterfaceId,
) -> Option<u16> {
    if context.usb_status.id() == id {
        return Some(USB_ENABLED);
    }
    if context.lora_status.is_some_and(|status| status.id() == id) {
        return Some(LORA_ENABLED);
    }
    if context.wifi_status.is_some_and(|status| status.id() == id) {
        return Some(WIFI_ENABLED);
    }
    if context
        .espnow_status
        .is_some_and(|status| status.id() == id)
    {
        return Some(ESPNOW_ENABLED);
    }
    if context.tcp_status.is_some_and(|status| status.id() == id) {
        return Some(TCP_ENABLED);
    }
    if id == BLE_SUPERVISOR_ID {
        return Some(BLUETOOTH_ENABLED);
    }
    None
}

fn desired_interface_enabled<D: S3DisplayRuntime>(
    context: &S3RemoteControlContext<'_, D>,
    id: InterfaceId,
) -> Option<bool> {
    interface_bit(context, id).map(|bit| context.system.0 & bit != 0)
}

fn set_desired_interface<D: S3DisplayRuntime>(
    context: &mut S3RemoteControlContext<'_, D>,
    id: InterfaceId,
    enabled: bool,
) {
    record_desired_interface(context, id, enabled);
    apply_interface_enabled(context, id, enabled);
}

fn record_desired_interface<D: S3DisplayRuntime>(
    context: &mut S3RemoteControlContext<'_, D>,
    id: InterfaceId,
    enabled: bool,
) {
    let Some(bit) = interface_bit(context, id) else {
        return;
    };
    context.system.set(bit, enabled);
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

fn set_embassy_status(status: &EmbassyInterfaceStatus, enabled: bool) {
    if enabled {
        status.enable();
    } else {
        status.disable();
    }
}

fn restore_desired_system<B: Esp32S3Board>(
    context: &S3RemoteControlContext<'_, impl S3DisplayRuntime>,
) {
    set_embassy_status(context.usb_status, context.system.0 & USB_ENABLED != 0);
    if let Some(status) = context.lora_status {
        set_embassy_status(status, context.system.0 & LORA_ENABLED != 0);
    }
    if let Some(status) = context.wifi_status {
        if context.system.0 & WIFI_ENABLED != 0 {
            status.enable();
        } else {
            status.disable();
        }
        if context.system.0 & STATION_UPLINK_ENABLED != 0 {
            status.enable_station_uplink();
        } else {
            status.disable_station_uplink();
        }
    }
    if let Some(status) = context.espnow_status {
        set_embassy_status(status, context.system.0 & ESPNOW_ENABLED != 0);
    }
    if let Some(status) = context.tcp_status {
        set_embassy_status(status, context.system.0 & TCP_ENABLED != 0);
    }
    let bluetooth = BluetoothAutoStatus::new(&BLE_SHARED);
    if context.system.0 & BLUETOOTH_ENABLED != 0 {
        bluetooth.enable();
    } else {
        bluetooth.disable();
    }
    if context.system.gnss_enabled() {
        B::Gnss::control(screen::GnssReceiverCommand::Enable);
    } else {
        B::Gnss::control(screen::GnssReceiverCommand::Disable);
    }
}

fn runtime_matches_desired(context: &S3RemoteControlContext<'_, impl S3DisplayRuntime>) -> bool {
    context.usb_status.is_enabled() == (context.system.0 & USB_ENABLED != 0)
        && context.lora_status.map_or(true, |status| {
            status.is_enabled() == (context.system.0 & LORA_ENABLED != 0)
        })
        && context.wifi_status.map_or(true, |status| {
            status.is_enabled() == (context.system.0 & WIFI_ENABLED != 0)
                && status.is_station_uplink_enabled()
                    == (context.system.0 & STATION_UPLINK_ENABLED != 0)
        })
        && context.espnow_status.map_or(true, |status| {
            status.is_enabled() == (context.system.0 & ESPNOW_ENABLED != 0)
        })
        && context.tcp_status.map_or(true, |status| {
            status.is_enabled() == (context.system.0 & TCP_ENABLED != 0)
        })
        && (BluetoothAutoStatus::new(&BLE_SHARED).is_enabled()
            == (context.system.0 & BLUETOOTH_ENABLED != 0))
}

pub(super) fn apply_scheduled<B: Esp32S3Board>(
    scheduled: &mut Option<ScheduledEffect>,
    usb_status: &EmbassyInterfaceStatus,
    lora_status: Option<&EmbassyInterfaceStatus>,
    wifi_status: Option<&AutoWifiStatus<MEMBERS>>,
    espnow_status: Option<&EmbassyInterfaceStatus>,
    tcp_status: Option<&EmbassyInterfaceStatus>,
    display: &mut impl S3DisplayRuntime,
    system: &mut SystemIntent,
    wifi_config: &mut HopspotWifiConfig,
    wifi_confirmation: &mut Option<WifiConfirmation>,
) {
    let Some(effect) = scheduled.as_ref() else {
        return;
    };
    if embassy_time::Instant::now() < effect.apply_at {
        return;
    }
    let Some(effect) = scheduled.take() else {
        return;
    };
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
        ScheduledAction::DisableStationUplink => {
            if let Some(status) = wifi_status {
                status.disable_station_uplink();
            }
        }
        ScheduledAction::ReconcileInterfaces => {
            set_embassy_status(usb_status, system.0 & USB_ENABLED != 0);
            if let Some(status) = lora_status {
                set_embassy_status(status, system.0 & LORA_ENABLED != 0);
            }
            if let Some(status) = wifi_status {
                if system.0 & WIFI_ENABLED != 0 {
                    status.enable();
                } else {
                    status.disable();
                }
                if system.0 & STATION_UPLINK_ENABLED != 0 {
                    status.enable_station_uplink();
                } else {
                    status.disable_station_uplink();
                }
            }
            if let Some(status) = espnow_status {
                set_embassy_status(status, system.0 & ESPNOW_ENABLED != 0);
            }
            if let Some(status) = tcp_status {
                set_embassy_status(status, system.0 & TCP_ENABLED != 0);
            }
            if system.0 & BLUETOOTH_ENABLED != 0 {
                BluetoothAutoStatus::new(&BLE_SHARED).enable();
            } else {
                BluetoothAutoStatus::new(&BLE_SHARED).disable();
            }
            if system.gnss_enabled() {
                B::Gnss::control(screen::GnssReceiverCommand::Enable);
            } else {
                B::Gnss::control(screen::GnssReceiverCommand::Disable);
            }
        }
        ScheduledAction::SleepSystem => {
            let now = display_now();
            if let Err(error) = display
                .schedule_blanking(now, DisplayBlankReason::SystemSleep)
                .and_then(|()| display.poll_blanking(now, display_now))
            {
                log::error!("scheduled system-sleep display apply failed: {error:?}");
                *scheduled = Some(ScheduledEffect {
                    action: ScheduledAction::SleepSystem,
                    apply_at: embassy_time::Instant::now() + RESPONSE_GRACE_PERIOD,
                });
                return;
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
            system.set_awake(false);
        }
        ScheduledAction::SetRadioMode(mode) => request_radio_mode(mode),
        ScheduledAction::ActivateWifi {
            controller,
            revision,
            station,
        } => {
            wifi_config.ssid = station.ssid().to_string();
            WIFI_CREDENTIALS.replace(screen::HopspotWifiCredentialUpdate {
                revision: Some(revision),
                station,
            });
            if let Some(status) = wifi_status {
                status.enable();
                status.enable_station_uplink();
            }
            system.set(WIFI_ENABLED, true);
            system.set(STATION_UPLINK_ENABLED, true);
            *wifi_confirmation = Some(WifiConfirmation {
                controller,
                revision,
                deadline: embassy_time::Instant::now()
                    + Duration::from_secs(u64::from(
                        REMOTE_CONTROL_WIFI_CONFIRMATION_WINDOW_SECONDS,
                    )),
            });
        }
        ScheduledAction::RestoreWifi { station } => {
            publish_resolved_wifi(station, None, wifi_config);
        }
    }
}

#[cfg(feature = "lora")]
pub(super) async fn apply_subg_configuration(
    controller: &mut personal_rns::lora::LoRaController<'static>,
    store: &mut screen::SubGConfigurationStore<S3SharedFlash>,
    active: &mut SubGConfigurationState,
    requested: SubGConfigurationState,
) -> Result<(), RemoteControlHostCommandError> {
    if *active == requested {
        return Ok(());
    }
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
            if controller.apply_configuration(previous).await != LoRaApplyOutcome::Applied {
                return Err(RemoteControlHostCommandError::RollbackFailed);
            }
            *active = previous;
            let rollback = match previous {
                SubGConfigurationState::Configured(configuration) => {
                    store.save(configuration).await
                }
                SubGConfigurationState::Unconfigured => store.clear().await,
            };
            if rollback == screen::SubGConfigurationCommitOutcome::Committed {
                Err(RemoteControlHostCommandError::PersistenceFailed)
            } else {
                Err(RemoteControlHostCommandError::RollbackFailed)
            }
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
    match persist_wifi_rollback(wifi_store, wifi_key, controller, revision, wifi_config).await {
        Ok(WifiRollbackOutcome::Committed(active)) => {
            match resolve_active_wifi(active) {
                Ok(station) => publish_resolved_wifi(station, None, wifi_config),
                Err(error) => {
                    log::error!("wifi-config: timed-out rollback provisioning invalid: {error:?}");
                    WIFI_CREDENTIALS.clear();
                    wifi_config.ssid.clear();
                    return;
                }
            }
            *wifi_confirmation = None;
            log::warn!("wifi-config: confirmation timed out; restored confirmed credentials");
        }
        Ok(WifiRollbackOutcome::Recovered) => {
            *wifi_confirmation = None;
            log::warn!("wifi-config: confirmation timed out; restored confirmed credentials");
        }
        Err(error) => log::error!("wifi-config: timed-out rollback failed: {error:?}"),
    }
}

enum WifiRollbackOutcome {
    Committed(Option<personal_rns::remote_control::RemoteControlWifiStation>),
    Recovered,
}

async fn persist_wifi_rollback(
    wifi_store: &mut screen::WifiConfigurationStore<S3SharedFlash>,
    wifi_key: &RemoteControlTargetSealingKey,
    controller: personal_rns::identity::IdentityHash,
    revision: RemoteControlWifiCredentialRevision,
    wifi_config: &mut HopspotWifiConfig,
) -> Result<WifiRollbackOutcome, RemoteControlHostCommandError> {
    let mut entropy = runtime_entropy();
    match wifi_store
        .cancel(controller, revision, wifi_key, &mut entropy)
        .await
    {
        screen::WifiConfigurationCommitOutcome::Committed(active) => {
            Ok(WifiRollbackOutcome::Committed(active))
        }
        screen::WifiConfigurationCommitOutcome::NotCommitted(
            screen::WifiConfigurationStoreError::ControllerMismatch
            | screen::WifiConfigurationStoreError::RevisionMismatch
            | screen::WifiConfigurationStoreError::NoTransaction,
        ) => Err(RemoteControlHostCommandError::ApplyFailed),
        screen::WifiConfigurationCommitOutcome::NotCommitted(_)
        | screen::WifiConfigurationCommitOutcome::Indeterminate(_) => {
            recover_wifi_store(wifi_store, wifi_key, wifi_config)
                .await
                .map(|_| WifiRollbackOutcome::Recovered)
        }
    }
}

async fn recover_wifi_store(
    wifi_store: &mut screen::WifiConfigurationStore<S3SharedFlash>,
    wifi_key: &RemoteControlTargetSealingKey,
    wifi_config: &mut HopspotWifiConfig,
) -> Result<screen::WifiConfigurationStatus, RemoteControlHostCommandError> {
    let mut entropy = runtime_entropy();
    let loaded = wifi_store
        .load(wifi_key, &mut entropy)
        .await
        .map_err(|_| RemoteControlHostCommandError::RollbackFailed)?;
    let status = loaded.status;
    let revision = match status {
        screen::WifiConfigurationStatus::Confirmed { revision } => Some(revision),
        screen::WifiConfigurationStatus::FactoryProvisioning => None,
        screen::WifiConfigurationStatus::Transaction { .. } => {
            return Err(RemoteControlHostCommandError::RollbackFailed);
        }
    };
    publish_active_wifi(loaded.active, revision, wifi_config)?;
    Ok(status)
}

fn publish_active_wifi(
    active: Option<personal_rns::remote_control::RemoteControlWifiStation>,
    revision: Option<RemoteControlWifiCredentialRevision>,
    wifi_config: &mut HopspotWifiConfig,
) -> Result<(), RemoteControlHostCommandError> {
    let station = resolve_active_wifi(active)?;
    publish_resolved_wifi(station, revision, wifi_config);
    Ok(())
}

fn resolve_active_wifi(
    active: Option<personal_rns::remote_control::RemoteControlWifiStation>,
) -> Result<
    Option<personal_rns::remote_control::RemoteControlWifiStation>,
    RemoteControlHostCommandError,
> {
    if active.is_some() {
        return Ok(active);
    }

    let (factory_wifi, _) = hopspot_wifi_config();
    if factory_wifi.has_station() {
        let station = personal_rns::remote_control::RemoteControlWifiStation::parse(
            &factory_wifi.ssid,
            &factory_wifi.password,
        )
        .map_err(|_| RemoteControlHostCommandError::RollbackFailed)?;
        Ok(Some(station))
    } else {
        Ok(None)
    }
}

fn publish_resolved_wifi(
    station: Option<personal_rns::remote_control::RemoteControlWifiStation>,
    revision: Option<RemoteControlWifiCredentialRevision>,
    wifi_config: &mut HopspotWifiConfig,
) {
    if let Some(station) = station {
        wifi_config.ssid = station.ssid().to_string();
        WIFI_CREDENTIALS.replace(screen::HopspotWifiCredentialUpdate { revision, station });
    } else {
        wifi_config.ssid.clear();
        WIFI_CREDENTIALS.clear();
    }
}
