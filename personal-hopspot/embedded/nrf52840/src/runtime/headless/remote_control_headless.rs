use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant, Timer};
use personal_hopspot_core as hopspot;
#[cfg(feature = "board-mesh-tower-v2")]
use personal_rns::bluetooth_auto::BluetoothAutoStatus;
use personal_rns::interfaces::subghz::{
    ResolvedSubGMode, SubGConfiguration, SubGConfigurationState,
};
use personal_rns::interfaces::{
    InterfaceGravity, InterfaceId, InterfaceMode, InterfaceSnapshot, InterfaceStatus, Membership,
};
use personal_rns::manifold::embassy::EmbassyInterfaceStatus;
#[cfg(feature = "board-t1000e")]
use personal_rns::remote_control::RemoteControlGnssPower;
use personal_rns::remote_control::{
    RemoteControlApplyOutcome, RemoteControlCapabilities, RemoteControlInterfacePower,
    RemoteControlLoRaOutcome, RemoteControlPowerOutcome, RemoteControlRequestKind,
    RemoteControlSystemPower,
};
#[cfg(feature = "board-mesh-tower-v2")]
use personal_rns::remote_control::{
    RemoteControlDiscoveryGroups, RemoteControlDiscoveryGroupsInventoryOutcome,
    RemoteControlDiscoveryGroupsReplaceOutcome, RemoteControlGroupOutcome,
};
use personal_rns::runtime::{
    RemoteControlHostCommand, RemoteControlHostCommandError, RemoteControlHostResponse,
};

#[cfg(feature = "board-t1000e")]
use crate::boards::selected as board;

#[cfg(feature = "board-mesh-tower-v2")]
use super::bluetooth::{BLE_SHARED, BLE_SUPERVISOR_ID, MEMBERS};
use super::{INTERFACE_STORE, REMOTE_CONTROL_COMMANDS};

const RESPONSE_GRACE_PERIOD: Duration = Duration::from_millis(250);
const LORA_ENABLED: u8 = 1 << 0;
const USB_ENABLED: u8 = 1 << 1;
#[cfg(feature = "board-mesh-tower-v2")]
const BLUETOOTH_ENABLED: u8 = 1 << 2;
#[cfg(feature = "board-mesh-tower-v2")]
const SNAPSHOT_CAPACITY: usize = MEMBERS + 3;
#[cfg(feature = "board-t1000e")]
const SNAPSHOT_CAPACITY: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScheduledAction {
    DisableInterface(InterfaceId),
    ReconcileInterfaces,
    SleepSystem,
}

struct ScheduledEffect {
    action: ScheduledAction,
    apply_at: Instant,
}

impl ScheduledEffect {
    fn deadline(&self) -> Instant {
        self.apply_at
    }
}

struct Context<'a> {
    snapshots: &'a [InterfaceSnapshot],
    lora_status: &'a EmbassyInterfaceStatus,
    usb_status: &'a EmbassyInterfaceStatus,
    system_awake: &'a mut bool,
    desired_interfaces: &'a mut u8,
    scheduled_effect: &'a mut Option<ScheduledEffect>,
    lora_controller: &'a mut personal_rns::lora::LoRaController<'static>,
    subg_configuration: &'a mut SubGConfigurationState,
    #[cfg(feature = "board-t1000e")]
    gnss_wanted: &'a mut bool,
}

pub(super) fn capabilities() -> RemoteControlCapabilities {
    let mut capabilities = RemoteControlCapabilities::describe_only();
    for kind in [
        RemoteControlRequestKind::AnnounceSelf,
        RemoteControlRequestKind::InventoryInterfaces,
        RemoteControlRequestKind::SetInterfacePower,
        RemoteControlRequestKind::InventoryInterfacePeers,
        RemoteControlRequestKind::InventoryInterfaceConfig,
        RemoteControlRequestKind::SetInterfaceLoRaProfile,
        RemoteControlRequestKind::DescribeBuild,
        RemoteControlRequestKind::SetSystemPower,
        RemoteControlRequestKind::InventoryControllers,
        RemoteControlRequestKind::AuthorizeController,
        RemoteControlRequestKind::RevokeController,
    ] {
        capabilities = capabilities.with_request(kind);
    }
    #[cfg(feature = "board-mesh-tower-v2")]
    for kind in [
        RemoteControlRequestKind::SetInterfaceGroup,
        RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups,
        RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups,
    ] {
        capabilities = capabilities.with_request(kind);
    }
    #[cfg(feature = "board-t1000e")]
    {
        capabilities = capabilities.with_request(RemoteControlRequestKind::SetGnssPower);
    }
    capabilities
}

pub(super) async fn run_headless(
    lora_status: &'static EmbassyInterfaceStatus,
    usb_status: &'static EmbassyInterfaceStatus,
    mut lora_controller: personal_rns::lora::LoRaController<'static>,
    mut subg_configuration: SubGConfigurationState,
) -> ! {
    let mut system_awake = true;
    let mut desired_interfaces = enabled_interfaces(lora_status, usb_status);
    let mut scheduled_effect: Option<ScheduledEffect> = None;
    #[cfg(feature = "board-t1000e")]
    let mut gnss_wanted = true;

    loop {
        let pending = match scheduled_effect.as_ref() {
            Some(effect) => match select(
                REMOTE_CONTROL_COMMANDS.receive(),
                Timer::at(effect.deadline()),
            )
            .await
            {
                Either::First(pending) => Some(pending),
                Either::Second(()) => {
                    apply_scheduled(
                        &mut scheduled_effect,
                        lora_status,
                        usb_status,
                        &mut system_awake,
                        desired_interfaces,
                    );
                    None
                }
            },
            None => Some(REMOTE_CONTROL_COMMANDS.receive().await),
        };
        let Some(pending) = pending else {
            continue;
        };
        let (token, command) = pending.into_parts();
        let result = match snapshots(lora_status, usb_status) {
            Ok(snapshots) => {
                execute(
                    Context {
                        snapshots: &snapshots,
                        lora_status,
                        usb_status,
                        system_awake: &mut system_awake,
                        desired_interfaces: &mut desired_interfaces,
                        scheduled_effect: &mut scheduled_effect,
                        lora_controller: &mut lora_controller,
                        subg_configuration: &mut subg_configuration,
                        #[cfg(feature = "board-t1000e")]
                        gnss_wanted: &mut gnss_wanted,
                    },
                    command,
                )
                .await
            }
            Err(error) => Err(error),
        };
        REMOTE_CONTROL_COMMANDS.complete(token, result);
    }
}

async fn execute(
    mut context: Context<'_>,
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
            #[cfg(feature = "board-mesh-tower-v2")]
            let ble_groups = BluetoothAutoStatus::new(&BLE_SHARED).discovery_groups();
            #[cfg(feature = "board-mesh-tower-v2")]
            let ble_group = hopspot::singleton_discovery_group(&ble_groups);
            #[cfg(not(feature = "board-mesh-tower-v2"))]
            let ble_group = None;
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
            let outcome = if desired && !*context.system_awake {
                return Err(RemoteControlHostCommandError::Busy);
            } else if !desired && !*context.system_awake {
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
        #[cfg(feature = "board-mesh-tower-v2")]
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
        #[cfg(feature = "board-mesh-tower-v2")]
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
        #[cfg(feature = "board-mesh-tower-v2")]
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
            let requested =
                SubGConfigurationState::Configured(SubGConfiguration::manual_lora(profile));
            if *context.subg_configuration != requested {
                if context.lora_controller.apply_configuration(requested).await
                    == personal_rns::lora::LoRaApplyOutcome::Rejected
                {
                    return Err(RemoteControlHostCommandError::ApplyFailed);
                }
                *context.subg_configuration = requested;
            }
            Ok(RemoteControlHostResponse::SetInterfaceLoRaProfile(
                RemoteControlLoRaOutcome::Applied,
            ))
        }
        RemoteControlHostCommand::DescribeBuild => Ok(RemoteControlHostResponse::DescribeBuild(
            hopspot::hopspot_remote_control_build_version()
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?,
        )),
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
            } else if *context.system_awake == desired_awake {
                RemoteControlApplyOutcome::Unchanged
            } else if desired_awake {
                restore_desired_interfaces(&context);
                #[cfg(feature = "board-t1000e")]
                if *context.gnss_wanted {
                    board::control_gnss(hopspot::GnssReceiverCommand::Enable);
                }
                *context.system_awake = true;
                RemoteControlApplyOutcome::Applied
            } else {
                schedule_effect(context.scheduled_effect, ScheduledAction::SleepSystem)?;
                RemoteControlApplyOutcome::Scheduled
            };
            Ok(RemoteControlHostResponse::SetSystemPower(outcome))
        }
        #[cfg(feature = "board-t1000e")]
        RemoteControlHostCommand::SetGnssPower { power } => {
            let desired = power == RemoteControlGnssPower::On;
            let outcome = if *context.gnss_wanted == desired {
                RemoteControlApplyOutcome::Unchanged
            } else {
                *context.gnss_wanted = desired;
                if *context.system_awake {
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

#[cfg(feature = "board-mesh-tower-v2")]
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

fn interface_bit(context: &Context<'_>, id: InterfaceId) -> Option<u8> {
    if context.lora_status.id() == id {
        Some(LORA_ENABLED)
    } else if context.usb_status.id() == id {
        Some(USB_ENABLED)
    } else {
        #[cfg(feature = "board-mesh-tower-v2")]
        if id == BLE_SUPERVISOR_ID {
            return Some(BLUETOOTH_ENABLED);
        }
        None
    }
}

fn desired_interface_enabled(context: &Context<'_>, id: InterfaceId) -> Option<bool> {
    interface_bit(context, id).map(|bit| *context.desired_interfaces & bit != 0)
}

fn set_desired_interface(context: &mut Context<'_>, id: InterfaceId, enabled: bool) {
    record_desired_interface(context, id, enabled);
    apply_interface_enabled(context, id, enabled);
}

fn record_desired_interface(context: &mut Context<'_>, id: InterfaceId, enabled: bool) {
    let Some(bit) = interface_bit(context, id) else {
        return;
    };
    if enabled {
        *context.desired_interfaces |= bit;
    } else {
        *context.desired_interfaces &= !bit;
    }
}

fn apply_interface_enabled(context: &Context<'_>, id: InterfaceId, enabled: bool) {
    if context.lora_status.id() == id {
        set_status(context.lora_status, enabled);
    } else if context.usb_status.id() == id {
        set_status(context.usb_status, enabled);
    } else {
        #[cfg(feature = "board-mesh-tower-v2")]
        if id == BLE_SUPERVISOR_ID {
            let status = BluetoothAutoStatus::new(&BLE_SHARED);
            if enabled {
                status.enable();
            } else {
                status.disable();
            }
        }
    }
}

fn restore_desired_interfaces(context: &Context<'_>) {
    set_status(
        context.lora_status,
        *context.desired_interfaces & LORA_ENABLED != 0,
    );
    set_status(
        context.usb_status,
        *context.desired_interfaces & USB_ENABLED != 0,
    );
    #[cfg(feature = "board-mesh-tower-v2")]
    {
        let bluetooth = BluetoothAutoStatus::new(&BLE_SHARED);
        if *context.desired_interfaces & BLUETOOTH_ENABLED != 0 {
            bluetooth.enable();
        } else {
            bluetooth.disable();
        }
    }
}

fn interfaces_match_desired(context: &Context<'_>) -> bool {
    context.lora_status.is_enabled() == (*context.desired_interfaces & LORA_ENABLED != 0)
        && context.usb_status.is_enabled() == (*context.desired_interfaces & USB_ENABLED != 0)
        && {
            #[cfg(feature = "board-mesh-tower-v2")]
            {
                BluetoothAutoStatus::new(&BLE_SHARED).is_enabled()
                    == (*context.desired_interfaces & BLUETOOTH_ENABLED != 0)
            }
            #[cfg(not(feature = "board-mesh-tower-v2"))]
            {
                true
            }
        }
}

fn enabled_interfaces(
    lora_status: &EmbassyInterfaceStatus,
    usb_status: &EmbassyInterfaceStatus,
) -> u8 {
    let mut enabled = 0;
    if lora_status.is_enabled() {
        enabled |= LORA_ENABLED;
    }
    if usb_status.is_enabled() {
        enabled |= USB_ENABLED;
    }
    #[cfg(feature = "board-mesh-tower-v2")]
    if BluetoothAutoStatus::new(&BLE_SHARED).is_enabled() {
        enabled |= BLUETOOTH_ENABLED;
    }
    enabled
}

fn set_status(status: &EmbassyInterfaceStatus, enabled: bool) {
    if enabled {
        status.enable();
    } else {
        status.disable();
    }
}

fn apply_scheduled(
    scheduled: &mut Option<ScheduledEffect>,
    lora_status: &EmbassyInterfaceStatus,
    usb_status: &EmbassyInterfaceStatus,
    system_awake: &mut bool,
    desired_interfaces: u8,
) {
    let Some(effect) = scheduled.take() else {
        return;
    };
    match effect.action {
        ScheduledAction::DisableInterface(id) => {
            if lora_status.id() == id {
                lora_status.disable();
            } else if usb_status.id() == id {
                usb_status.disable();
            } else {
                #[cfg(feature = "board-mesh-tower-v2")]
                if id == BLE_SUPERVISOR_ID {
                    BluetoothAutoStatus::new(&BLE_SHARED).disable();
                }
            }
        }
        ScheduledAction::ReconcileInterfaces => {
            set_status(lora_status, desired_interfaces & LORA_ENABLED != 0);
            set_status(usb_status, desired_interfaces & USB_ENABLED != 0);
            #[cfg(feature = "board-mesh-tower-v2")]
            if desired_interfaces & BLUETOOTH_ENABLED != 0 {
                BluetoothAutoStatus::new(&BLE_SHARED).enable();
            } else {
                BluetoothAutoStatus::new(&BLE_SHARED).disable();
            }
        }
        ScheduledAction::SleepSystem => {
            lora_status.disable();
            usb_status.disable();
            #[cfg(feature = "board-mesh-tower-v2")]
            BluetoothAutoStatus::new(&BLE_SHARED).disable();
            #[cfg(feature = "board-t1000e")]
            board::control_gnss(hopspot::GnssReceiverCommand::Disable);
            *system_awake = false;
        }
    }
}

fn snapshots(
    lora: &EmbassyInterfaceStatus,
    usb: &EmbassyInterfaceStatus,
) -> Result<heapless::Vec<InterfaceSnapshot, SNAPSHOT_CAPACITY>, RemoteControlHostCommandError> {
    #[cfg(feature = "board-mesh-tower-v2")]
    let bluetooth = BluetoothAutoStatus::new(&BLE_SHARED);
    let mut entries: heapless::Vec<(&dyn InterfaceStatus, Membership), SNAPSHOT_CAPACITY> =
        heapless::Vec::new();
    entries
        .push((lora, Membership::Independent))
        .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
    entries
        .push((usb, Membership::Independent))
        .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
    #[cfg(feature = "board-mesh-tower-v2")]
    {
        let supervisor_id = bluetooth.id();
        entries
            .push((&bluetooth, Membership::Independent))
            .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
        for member in bluetooth.members() {
            entries
                .push((member, Membership::FleetMember { supervisor_id }))
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
        }
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
            .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
    }
    Ok(snapshots)
}
