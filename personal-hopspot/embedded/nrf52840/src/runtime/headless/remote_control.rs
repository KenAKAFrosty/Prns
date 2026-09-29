use embassy_time::{Duration, Instant};
use personal_hopspot_core as hopspot;
use personal_rns::bluetooth_auto::BluetoothAutoStatus;
use personal_rns::interfaces::subghz::{
    ResolvedSubGMode, SubGConfiguration, SubGConfigurationState,
};
use personal_rns::interfaces::{InterfaceId, InterfaceSnapshot, InterfaceStatus};
use personal_rns::manifold::embassy::EmbassyInterfaceStatus;
#[cfg(feature = "board-t096")]
use personal_rns::remote_control::RemoteControlGnssPower;
use personal_rns::remote_control::{
    RemoteControlApplyOutcome, RemoteControlCapabilities, RemoteControlDiscoveryGroups,
    RemoteControlDiscoveryGroupsInventoryOutcome, RemoteControlDiscoveryGroupsReplaceOutcome,
    RemoteControlDisplayAutoOff, RemoteControlDisplayVisibility, RemoteControlGroupOutcome,
    RemoteControlInterfacePower, RemoteControlLoRaOutcome, RemoteControlPowerOutcome,
    RemoteControlRequestKind, RemoteControlSystemPower,
};
use personal_rns::runtime::{
    RemoteControlHostCommand, RemoteControlHostCommandError, RemoteControlHostResponse,
};

#[cfg(feature = "board-t096")]
use crate::boards::selected as board;
use crate::immediate_display::{ImmediateDisplayDevice, ImmediateDisplayRuntime};

use super::bluetooth::{BLE_SHARED, BLE_SUPERVISOR_ID};

const RESPONSE_GRACE_PERIOD: Duration = Duration::from_millis(250);
const SYSTEM_AWAKE: u8 = 1 << 0;
const LORA_ENABLED: u8 = 1 << 1;
const USB_ENABLED: u8 = 1 << 2;
const BLUETOOTH_ENABLED: u8 = 1 << 3;

pub(super) struct SystemIntent(u8);

impl SystemIntent {
    pub(super) fn from_status(
        lora_status: &EmbassyInterfaceStatus,
        usb_status: &EmbassyInterfaceStatus,
    ) -> Self {
        let mut bits = SYSTEM_AWAKE;
        if lora_status.is_enabled() {
            bits |= LORA_ENABLED;
        }
        if usb_status.is_enabled() {
            bits |= USB_ENABLED;
        }
        if BluetoothAutoStatus::new(&BLE_SHARED).is_enabled() {
            bits |= BLUETOOTH_ENABLED;
        }
        Self(bits)
    }

    fn is_awake(&self) -> bool {
        self.0 & SYSTEM_AWAKE != 0
    }

    fn set_awake(&mut self, awake: bool) {
        if awake {
            self.0 |= SYSTEM_AWAKE;
        } else {
            self.0 &= !SYSTEM_AWAKE;
        }
    }
}

type ConfigurationStore = hopspot::SubGConfigurationStore<super::super::learned_state::BoardFlash>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScheduledAction {
    DisableInterface(InterfaceId),
    ReconcileInterfaces,
    SleepSystem,
}

pub(super) struct ScheduledEffect {
    action: ScheduledAction,
    apply_at: Instant,
}

pub(super) struct Context<'a, D: ImmediateDisplayDevice> {
    pub snapshots: &'a [InterfaceSnapshot],
    pub lora_status: &'a EmbassyInterfaceStatus,
    pub usb_status: &'a EmbassyInterfaceStatus,
    pub display: &'a mut ImmediateDisplayRuntime<D>,
    pub power: hopspot::PowerSnapshot,
    pub system: &'a mut SystemIntent,
    pub scheduled_effect: &'a mut Option<ScheduledEffect>,
    pub lora_controller: &'a mut personal_rns::lora::LoRaController<'static>,
    pub subg_store: &'a mut ConfigurationStore,
    pub subg_configuration: &'a mut SubGConfigurationState,
    #[cfg(feature = "board-t096")]
    pub gnss_wanted: &'a mut bool,
}

pub(super) fn capabilities() -> RemoteControlCapabilities {
    let mut capabilities = RemoteControlCapabilities::describe_only();
    for kind in [
        RemoteControlRequestKind::AnnounceSelf,
        RemoteControlRequestKind::InventoryInterfaces,
        RemoteControlRequestKind::SetInterfacePower,
        RemoteControlRequestKind::SetInterfaceGroup,
        RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups,
        RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups,
        RemoteControlRequestKind::InventoryInterfacePeers,
        RemoteControlRequestKind::InventoryInterfaceConfig,
        RemoteControlRequestKind::SetInterfaceLoRaProfile,
        RemoteControlRequestKind::DescribeBuild,
        RemoteControlRequestKind::DescribePower,
        RemoteControlRequestKind::SetSystemPower,
        RemoteControlRequestKind::SetDisplayVisibility,
        RemoteControlRequestKind::SetDisplayAutoOff,
        RemoteControlRequestKind::InventoryControllers,
        RemoteControlRequestKind::AuthorizeController,
        RemoteControlRequestKind::RevokeController,
    ] {
        capabilities = capabilities.with_request(kind);
    }
    #[cfg(feature = "board-t096")]
    {
        capabilities = capabilities.with_request(RemoteControlRequestKind::SetGnssPower);
    }
    capabilities
}

pub(super) async fn execute<D: ImmediateDisplayDevice>(
    mut context: Context<'_, D>,
    command: RemoteControlHostCommand,
) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError> {
    match command {
        RemoteControlHostCommand::InventoryInterfaces { page } => {
            Ok(RemoteControlHostResponse::InventoryInterfaces(
                hopspot::remote_control_inventory_from_snapshots(context.snapshots, page)
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?,
            ))
        }
        RemoteControlHostCommand::InventoryInterfacePeers { id, page } => {
            Ok(RemoteControlHostResponse::InventoryInterfacePeers(
                hopspot::remote_control_interface_peers_from_snapshots(context.snapshots, id, page)
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?,
            ))
        }
        RemoteControlHostCommand::InventoryInterfaceConfig { id } => {
            let profile = match *context.subg_configuration {
                SubGConfigurationState::Configured(configuration) => {
                    let ResolvedSubGMode::LoRa(profile) = configuration.resolve();
                    Some(profile)
                }
                SubGConfigurationState::Unconfigured => None,
            };
            let ble_groups = BluetoothAutoStatus::new(&BLE_SHARED).discovery_groups();
            let ble_group = hopspot::singleton_discovery_group(&ble_groups);
            let outcome = hopspot::remote_control_interface_config_from_snapshots(
                context.snapshots,
                id,
                |snapshot, card| {
                    hopspot::decorate_hopspot_remote_control_card(
                        snapshot, card, ble_group, profile, None, None,
                    )
                },
            )
            .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
            Ok(RemoteControlHostResponse::InventoryInterfaceConfig(outcome))
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
        RemoteControlHostCommand::SetInterfaceGroup { id, group } => {
            let groups = personal_rns::interfaces::DiscoveryGroupSet::from_singleton(group);
            let outcome = replace_bluetooth_discovery_groups(id, &groups).await?;
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
            } else {
                RemoteControlDiscoveryGroupsInventoryOutcome::UnknownInterface
            };
            Ok(RemoteControlHostResponse::InventoryInterfaceDiscoveryGroups(outcome))
        }
        RemoteControlHostCommand::ReplaceInterfaceDiscoveryGroups { id, groups } => {
            let groups = groups.into_groups();
            Ok(RemoteControlHostResponse::ReplaceInterfaceDiscoveryGroups(
                replace_bluetooth_discovery_groups(id, &groups).await?,
            ))
        }
        RemoteControlHostCommand::SetInterfaceLoRaProfile { id, profile } => {
            if context.lora_status.id() != id {
                return Ok(RemoteControlHostResponse::SetInterfaceLoRaProfile(
                    RemoteControlLoRaOutcome::UnknownInterface,
                ));
            }
            let profile = profile
                .profile()
                .ok_or(RemoteControlHostCommandError::ApplyFailed)?;
            apply_subg_configuration(
                context.lora_controller,
                context.subg_store,
                context.subg_configuration,
                SubGConfigurationState::Configured(SubGConfiguration::manual_lora(profile)),
            )
            .await?;
            Ok(RemoteControlHostResponse::SetInterfaceLoRaProfile(
                RemoteControlLoRaOutcome::Applied,
            ))
        }
        RemoteControlHostCommand::DescribeBuild => Ok(RemoteControlHostResponse::DescribeBuild(
            hopspot::hopspot_remote_control_build_version()
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?,
        )),
        RemoteControlHostCommand::DescribePower => {
            Ok(RemoteControlHostResponse::DescribePower(context.power))
        }
        RemoteControlHostCommand::SetSystemPower { power } => {
            let desired_awake = power == RemoteControlSystemPower::Awake;
            let outcome = if desired_awake && cancel_pending_sleep(context.scheduled_effect) {
                if interfaces_match_desired(&context) {
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
                    .await
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
                restore_desired_interfaces(&context);
                #[cfg(feature = "board-t096")]
                if *context.gnss_wanted {
                    board::control_gnss(hopspot::GnssReceiverCommand::Enable);
                }
                context.system.set_awake(true);
                RemoteControlApplyOutcome::Applied
            } else {
                schedule_effect(context.scheduled_effect, ScheduledAction::SleepSystem)?;
                RemoteControlApplyOutcome::Scheduled
            };
            Ok(RemoteControlHostResponse::SetSystemPower(outcome))
        }
        RemoteControlHostCommand::SetDisplayVisibility { visibility } => {
            if context.display.visibility() == hopspot::display::DisplayVisibility::Unavailable {
                return Err(RemoteControlHostCommandError::Unsupported);
            }
            let desired_visible = visibility == RemoteControlDisplayVisibility::Visible;
            if desired_visible && !context.system.is_awake() {
                return Err(RemoteControlHostCommandError::Busy);
            }
            let currently_visible =
                context.display.visibility() == hopspot::display::DisplayVisibility::Visible;
            let outcome = if desired_visible == currently_visible {
                RemoteControlApplyOutcome::Unchanged
            } else {
                if desired_visible {
                    context
                        .display
                        .request_visible(display_now(), display_now)
                        .await
                        .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
                } else {
                    blank_display(
                        context.display,
                        hopspot::display::DisplayBlankReason::DisplayOnly,
                    )
                    .await?;
                }
                RemoteControlApplyOutcome::Applied
            };
            Ok(RemoteControlHostResponse::SetDisplayVisibility(outcome))
        }
        RemoteControlHostCommand::SetDisplayAutoOff { auto_off } => {
            let desired = match auto_off {
                RemoteControlDisplayAutoOff::Enabled => hopspot::display::DisplayAutoOff::Enabled,
                RemoteControlDisplayAutoOff::Disabled => hopspot::display::DisplayAutoOff::Disabled,
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
        #[cfg(feature = "board-t096")]
        RemoteControlHostCommand::SetGnssPower { power } => {
            let desired = power == RemoteControlGnssPower::On;
            let outcome = if *context.gnss_wanted == desired {
                RemoteControlApplyOutcome::Unchanged
            } else {
                *context.gnss_wanted = desired;
                if context.system.is_awake() {
                    board::control_gnss(if desired {
                        hopspot::GnssReceiverCommand::Enable
                    } else {
                        hopspot::GnssReceiverCommand::Disable
                    });
                }
                RemoteControlApplyOutcome::Applied
            };
            Ok(RemoteControlHostResponse::SetGnssPower(outcome))
        }
        _ => Err(RemoteControlHostCommandError::Unsupported),
    }
}

async fn replace_bluetooth_discovery_groups(
    id: InterfaceId,
    desired: &personal_rns::interfaces::DiscoveryGroupSet,
) -> Result<RemoteControlDiscoveryGroupsReplaceOutcome, RemoteControlHostCommandError> {
    if id != BLE_SUPERVISOR_ID {
        return Ok(RemoteControlDiscoveryGroupsReplaceOutcome::UnknownInterface);
    }
    let status = BluetoothAutoStatus::new(&BLE_SHARED);
    if status.discovery_groups() == *desired {
        return Ok(RemoteControlDiscoveryGroupsReplaceOutcome::Unchanged);
    }
    let prepared = hopspot::persist_discovery_group_replacement(id, desired).await?;
    if matches!(
        status.replace_discovery_groups(desired).await,
        personal_rns::interfaces::DiscoveryGroupApplyOutcome::Applied
            | personal_rns::interfaces::DiscoveryGroupApplyOutcome::Unchanged
    ) {
        return Ok(RemoteControlDiscoveryGroupsReplaceOutcome::Applied);
    }
    let durable_restored = hopspot::rollback_discovery_group_replacement(&prepared)
        .await
        .is_ok();
    let previous = prepared.runtime_rollback_groups();
    let runtime_restored = status.discovery_groups() == previous
        || matches!(
            status.replace_discovery_groups(&previous).await,
            personal_rns::interfaces::DiscoveryGroupApplyOutcome::Applied
                | personal_rns::interfaces::DiscoveryGroupApplyOutcome::Unchanged
        );
    if !durable_restored || !runtime_restored {
        return Err(RemoteControlHostCommandError::RollbackFailed);
    }
    Err(RemoteControlHostCommandError::ApplyFailed)
}

fn cancel_pending_interface_disable(
    scheduled: &mut Option<ScheduledEffect>,
    id: InterfaceId,
) -> bool {
    if scheduled
        .as_ref()
        .is_some_and(|effect| effect.action == ScheduledAction::DisableInterface(id))
    {
        *scheduled = None;
        true
    } else {
        false
    }
}

fn cancel_pending_sleep(scheduled: &mut Option<ScheduledEffect>) -> bool {
    if scheduled
        .as_ref()
        .is_some_and(|effect| effect.action == ScheduledAction::SleepSystem)
    {
        *scheduled = None;
        true
    } else {
        false
    }
}

fn has_pending_sleep(scheduled: &Option<ScheduledEffect>) -> bool {
    scheduled
        .as_ref()
        .is_some_and(|effect| effect.action == ScheduledAction::SleepSystem)
}

fn display_now() -> hopspot::display::MonotonicMillis {
    hopspot::display::MonotonicMillis::new(Instant::now().as_millis())
}

async fn blank_display<D: ImmediateDisplayDevice>(
    display: &mut ImmediateDisplayRuntime<D>,
    reason: hopspot::display::DisplayBlankReason,
) -> Result<(), RemoteControlHostCommandError> {
    let now = display_now();
    display
        .schedule_blanking(now, reason)
        .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
    display
        .poll_blanking(now, display_now)
        .await
        .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
    Ok(())
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
        apply_at: Instant::now() + RESPONSE_GRACE_PERIOD,
    });
    Ok(())
}

fn interface_bit<D: ImmediateDisplayDevice>(
    context: &Context<'_, D>,
    id: InterfaceId,
) -> Option<u8> {
    if context.lora_status.id() == id {
        Some(LORA_ENABLED)
    } else if context.usb_status.id() == id {
        Some(USB_ENABLED)
    } else if id == BLE_SUPERVISOR_ID {
        Some(BLUETOOTH_ENABLED)
    } else {
        None
    }
}

fn desired_interface_enabled<D: ImmediateDisplayDevice>(
    context: &Context<'_, D>,
    id: InterfaceId,
) -> Option<bool> {
    interface_bit(context, id).map(|bit| context.system.0 & bit != 0)
}

fn set_desired_interface<D: ImmediateDisplayDevice>(
    context: &mut Context<'_, D>,
    id: InterfaceId,
    enabled: bool,
) {
    record_desired_interface(context, id, enabled);
    apply_interface_enabled(context, id, enabled);
}

fn record_desired_interface<D: ImmediateDisplayDevice>(
    context: &mut Context<'_, D>,
    id: InterfaceId,
    enabled: bool,
) {
    let Some(bit) = interface_bit(context, id) else {
        return;
    };
    if enabled {
        context.system.0 |= bit;
    } else {
        context.system.0 &= !bit;
    }
}

fn apply_interface_enabled<D: ImmediateDisplayDevice>(
    context: &Context<'_, D>,
    id: InterfaceId,
    enabled: bool,
) {
    if context.lora_status.id() == id {
        set_status(context.lora_status, enabled);
    } else if context.usb_status.id() == id {
        set_status(context.usb_status, enabled);
    } else if id == BLE_SUPERVISOR_ID {
        let status = BluetoothAutoStatus::new(&BLE_SHARED);
        if enabled {
            status.enable();
        } else {
            status.disable();
        }
    }
}

fn restore_desired_interfaces<D: ImmediateDisplayDevice>(context: &Context<'_, D>) {
    set_status(context.lora_status, context.system.0 & LORA_ENABLED != 0);
    set_status(context.usb_status, context.system.0 & USB_ENABLED != 0);
    let bluetooth = BluetoothAutoStatus::new(&BLE_SHARED);
    if context.system.0 & BLUETOOTH_ENABLED != 0 {
        bluetooth.enable();
    } else {
        bluetooth.disable();
    }
}

fn interfaces_match_desired<D: ImmediateDisplayDevice>(context: &Context<'_, D>) -> bool {
    context.lora_status.is_enabled() == (context.system.0 & LORA_ENABLED != 0)
        && context.usb_status.is_enabled() == (context.system.0 & USB_ENABLED != 0)
        && (BluetoothAutoStatus::new(&BLE_SHARED).is_enabled()
            == (context.system.0 & BLUETOOTH_ENABLED != 0))
}

fn set_status(status: &EmbassyInterfaceStatus, enabled: bool) {
    if enabled {
        status.enable();
    } else {
        status.disable();
    }
}

pub(super) async fn apply_scheduled<D: ImmediateDisplayDevice>(
    scheduled: &mut Option<ScheduledEffect>,
    lora_status: &EmbassyInterfaceStatus,
    usb_status: &EmbassyInterfaceStatus,
    display: &mut ImmediateDisplayRuntime<D>,
    system: &mut SystemIntent,
) {
    let Some(effect) = scheduled.as_ref() else {
        return;
    };
    if Instant::now() < effect.apply_at {
        return;
    }
    let Some(effect) = scheduled.take() else {
        return;
    };
    match effect.action {
        ScheduledAction::DisableInterface(id) => {
            if lora_status.id() == id {
                lora_status.disable();
            } else if usb_status.id() == id {
                usb_status.disable();
            } else if id == BLE_SUPERVISOR_ID {
                BluetoothAutoStatus::new(&BLE_SHARED).disable();
            }
        }
        ScheduledAction::ReconcileInterfaces => {
            set_status(lora_status, system.0 & LORA_ENABLED != 0);
            set_status(usb_status, system.0 & USB_ENABLED != 0);
            if system.0 & BLUETOOTH_ENABLED != 0 {
                BluetoothAutoStatus::new(&BLE_SHARED).enable();
            } else {
                BluetoothAutoStatus::new(&BLE_SHARED).disable();
            }
        }
        ScheduledAction::SleepSystem => {
            if blank_display(display, hopspot::display::DisplayBlankReason::SystemSleep)
                .await
                .is_err()
            {
                *scheduled = Some(ScheduledEffect {
                    action: ScheduledAction::SleepSystem,
                    apply_at: Instant::now() + RESPONSE_GRACE_PERIOD,
                });
                return;
            }
            lora_status.disable();
            usb_status.disable();
            BluetoothAutoStatus::new(&BLE_SHARED).disable();
            #[cfg(feature = "board-t096")]
            board::control_gnss(hopspot::GnssReceiverCommand::Disable);
            system.set_awake(false);
        }
    }
}

pub(super) async fn apply_subg_configuration(
    controller: &mut personal_rns::lora::LoRaController<'static>,
    store: &mut ConfigurationStore,
    active: &mut SubGConfigurationState,
    requested: SubGConfigurationState,
) -> Result<(), RemoteControlHostCommandError> {
    if *active == requested {
        return Ok(());
    }
    let previous = *active;
    if controller.apply_configuration(requested).await
        == personal_rns::lora::LoRaApplyOutcome::Rejected
    {
        return Err(RemoteControlHostCommandError::ApplyFailed);
    }
    *active = requested;
    let persistence = match requested {
        SubGConfigurationState::Configured(configuration) => store.save(configuration).await,
        SubGConfigurationState::Unconfigured => store.clear().await,
    };
    match persistence {
        hopspot::SubGConfigurationCommitOutcome::Committed => Ok(()),
        hopspot::SubGConfigurationCommitOutcome::Indeterminate(_) => {
            if controller.apply_configuration(previous).await
                != personal_rns::lora::LoRaApplyOutcome::Applied
            {
                return Err(RemoteControlHostCommandError::RollbackFailed);
            }
            *active = previous;
            let rollback = match previous {
                SubGConfigurationState::Configured(configuration) => {
                    store.save(configuration).await
                }
                SubGConfigurationState::Unconfigured => store.clear().await,
            };
            if rollback == hopspot::SubGConfigurationCommitOutcome::Committed {
                Err(RemoteControlHostCommandError::PersistenceFailed)
            } else {
                Err(RemoteControlHostCommandError::RollbackFailed)
            }
        }
        hopspot::SubGConfigurationCommitOutcome::NotCommitted(_) => {
            if controller.apply_configuration(previous).await
                == personal_rns::lora::LoRaApplyOutcome::Applied
            {
                *active = previous;
                Err(RemoteControlHostCommandError::PersistenceFailed)
            } else {
                Err(RemoteControlHostCommandError::RollbackFailed)
            }
        }
    }
}
