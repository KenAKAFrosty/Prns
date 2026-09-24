use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant, Timer};
use personal_rns::interfaces::{
    InterfaceGravity, InterfaceId, InterfaceMode, InterfaceSnapshot, InterfaceStatus, Membership,
};
use personal_rns::manifold::embassy::EmbassyInterfaceStatus;
use personal_rns::remote_control::{
    RemoteControlApplyOutcome, RemoteControlCapabilities, RemoteControlInterfacePower,
    RemoteControlPowerOutcome, RemoteControlRequestKind, RemoteControlSystemPower,
};
#[cfg(feature = "bluetooth-auto")]
use personal_rns::remote_control::{
    RemoteControlDiscoveryGroups, RemoteControlDiscoveryGroupsInventoryOutcome,
    RemoteControlDiscoveryGroupsReplaceOutcome, RemoteControlGroupOutcome,
};
use personal_rns::runtime::{
    RemoteControlHostCommand, RemoteControlHostCommandError, RemoteControlHostResponse,
};

use super::*;

const RESPONSE_GRACE_PERIOD: Duration = Duration::from_millis(250);
const USB_ENABLED: u8 = 1 << 0;
const ESPNOW_ENABLED: u8 = 1 << 1;
#[cfg(feature = "bluetooth-auto")]
const BLUETOOTH_ENABLED: u8 = 1 << 2;

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
    usb_status: &'a EmbassyInterfaceStatus,
    espnow_status: Option<&'a EmbassyInterfaceStatus>,
    system_awake: &'a mut bool,
    desired_interfaces: &'a mut u8,
    scheduled_effect: &'a mut Option<ScheduledEffect>,
}

pub(super) fn capabilities() -> RemoteControlCapabilities {
    let mut capabilities = RemoteControlCapabilities::describe_only();
    for kind in [
        RemoteControlRequestKind::AnnounceSelf,
        RemoteControlRequestKind::InventoryInterfaces,
        RemoteControlRequestKind::SetInterfacePower,
        RemoteControlRequestKind::InventoryInterfacePeers,
        RemoteControlRequestKind::InventoryInterfaceConfig,
        RemoteControlRequestKind::DescribeBuild,
        RemoteControlRequestKind::InventoryPathTable,
        RemoteControlRequestKind::SetSystemPower,
        RemoteControlRequestKind::InventoryControllers,
        RemoteControlRequestKind::AuthorizeController,
        RemoteControlRequestKind::RevokeController,
    ] {
        capabilities = capabilities.with_request(kind);
    }
    #[cfg(feature = "bluetooth-auto")]
    for kind in [
        RemoteControlRequestKind::SetInterfaceGroup,
        RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups,
        RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups,
    ] {
        capabilities = capabilities.with_request(kind);
    }
    capabilities
}

pub(super) async fn run(
    usb_status: &'static EmbassyInterfaceStatus,
    espnow_status: Option<&'static EmbassyInterfaceStatus>,
) -> ! {
    let mut system_awake = true;
    let mut desired_interfaces = enabled_interfaces(usb_status, espnow_status);
    let mut scheduled_effect: Option<ScheduledEffect> = None;
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
                        usb_status,
                        espnow_status,
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
        let result = match snapshots(usb_status, espnow_status) {
            Ok(snapshots) => {
                execute(
                    Context {
                        snapshots: &snapshots,
                        usb_status,
                        espnow_status,
                        system_awake: &mut system_awake,
                        desired_interfaces: &mut desired_interfaces,
                        scheduled_effect: &mut scheduled_effect,
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
                personal_hopspot_core::remote_control_inventory_from_snapshots(
                    context.snapshots,
                    page,
                )
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?,
            ))
        }
        RemoteControlHostCommand::InventoryInterfacePeers { id, page } => {
            Ok(RemoteControlHostResponse::InventoryInterfacePeers(
                personal_hopspot_core::remote_control_interface_peers_from_snapshots(
                    context.snapshots,
                    id,
                    page,
                )
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?,
            ))
        }
        RemoteControlHostCommand::InventoryInterfaceConfig { id } => {
            #[cfg(feature = "bluetooth-auto")]
            let ble_groups = BluetoothAutoStatus::new(&BLE_SHARED).discovery_groups();
            #[cfg(feature = "bluetooth-auto")]
            let ble_group = personal_hopspot_core::singleton_discovery_group(&ble_groups);
            #[cfg(not(feature = "bluetooth-auto"))]
            let ble_group = None;
            let outcome = personal_hopspot_core::remote_control_interface_config_from_snapshots(
                context.snapshots,
                id,
                |snapshot, card| {
                    personal_hopspot_core::decorate_hopspot_remote_control_card(
                        snapshot, card, ble_group, None, None, None,
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
        #[cfg(feature = "bluetooth-auto")]
        RemoteControlHostCommand::SetInterfaceGroup { id, group } => {
            let groups = personal_rns::interfaces::DiscoveryGroupSet::from_singleton(group);
            let outcome = replace_bluetooth_discovery_groups(id, groups).await?;
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
        #[cfg(feature = "bluetooth-auto")]
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
        #[cfg(feature = "bluetooth-auto")]
        RemoteControlHostCommand::ReplaceInterfaceDiscoveryGroups { id, groups } => {
            Ok(RemoteControlHostResponse::ReplaceInterfaceDiscoveryGroups(
                replace_bluetooth_discovery_groups(id, groups.into_groups()).await?,
            ))
        }
        RemoteControlHostCommand::DescribeBuild => Ok(RemoteControlHostResponse::DescribeBuild(
            personal_hopspot_core::hopspot_remote_control_build_version()
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
                *context.system_awake = true;
                RemoteControlApplyOutcome::Applied
            } else {
                schedule_effect(context.scheduled_effect, ScheduledAction::SleepSystem)?;
                RemoteControlApplyOutcome::Scheduled
            };
            Ok(RemoteControlHostResponse::SetSystemPower(outcome))
        }
        _ => Err(RemoteControlHostCommandError::Unsupported),
    }
}

#[cfg(feature = "bluetooth-auto")]
async fn replace_bluetooth_discovery_groups(
    id: InterfaceId,
    desired: personal_rns::interfaces::DiscoveryGroupSet,
) -> Result<RemoteControlDiscoveryGroupsReplaceOutcome, RemoteControlHostCommandError> {
    if id != BLE_SUPERVISOR_ID {
        return Ok(RemoteControlDiscoveryGroupsReplaceOutcome::UnknownInterface);
    }
    let status = BluetoothAutoStatus::new(&BLE_SHARED);
    let previous = status.discovery_groups();
    if previous == desired {
        return Ok(RemoteControlDiscoveryGroupsReplaceOutcome::Unchanged);
    }
    let prepared = personal_hopspot_core::persist_discovery_group_replacement(id, &desired).await?;
    if matches!(
        status.replace_discovery_groups(&desired).await,
        personal_rns::interfaces::DiscoveryGroupApplyOutcome::Applied
            | personal_rns::interfaces::DiscoveryGroupApplyOutcome::Unchanged
    ) {
        return Ok(RemoteControlDiscoveryGroupsReplaceOutcome::Applied);
    }
    let durable_restored = personal_hopspot_core::rollback_discovery_group_replacement(&prepared)
        .await
        .is_ok();
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
    if context.usb_status.id() == id {
        Some(USB_ENABLED)
    } else if context
        .espnow_status
        .is_some_and(|status| status.id() == id)
    {
        Some(ESPNOW_ENABLED)
    } else {
        #[cfg(feature = "bluetooth-auto")]
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
    if context.usb_status.id() == id {
        set_status(context.usb_status, enabled);
    } else if let Some(status) = context.espnow_status.filter(|status| status.id() == id) {
        set_status(status, enabled);
    } else {
        #[cfg(feature = "bluetooth-auto")]
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
        context.usb_status,
        *context.desired_interfaces & USB_ENABLED != 0,
    );
    if let Some(status) = context.espnow_status {
        set_status(status, *context.desired_interfaces & ESPNOW_ENABLED != 0);
    }
    #[cfg(feature = "bluetooth-auto")]
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
    context.usb_status.is_enabled() == (*context.desired_interfaces & USB_ENABLED != 0)
        && context.espnow_status.is_none_or(|status| {
            status.is_enabled() == (*context.desired_interfaces & ESPNOW_ENABLED != 0)
        })
        && {
            #[cfg(feature = "bluetooth-auto")]
            {
                BluetoothAutoStatus::new(&BLE_SHARED).is_enabled()
                    == (*context.desired_interfaces & BLUETOOTH_ENABLED != 0)
            }
            #[cfg(not(feature = "bluetooth-auto"))]
            {
                true
            }
        }
}

fn enabled_interfaces(
    usb_status: &EmbassyInterfaceStatus,
    espnow_status: Option<&EmbassyInterfaceStatus>,
) -> u8 {
    let mut enabled = if usb_status.is_enabled() {
        USB_ENABLED
    } else {
        0
    };
    if espnow_status.is_some_and(EmbassyInterfaceStatus::is_enabled) {
        enabled |= ESPNOW_ENABLED;
    }
    #[cfg(feature = "bluetooth-auto")]
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
    usb_status: &EmbassyInterfaceStatus,
    espnow_status: Option<&EmbassyInterfaceStatus>,
    system_awake: &mut bool,
    desired_interfaces: u8,
) {
    let Some(effect) = scheduled.take() else {
        return;
    };
    match effect.action {
        ScheduledAction::DisableInterface(id) => {
            if usb_status.id() == id {
                usb_status.disable();
            } else if let Some(status) = espnow_status.filter(|status| status.id() == id) {
                status.disable();
            } else {
                #[cfg(feature = "bluetooth-auto")]
                if id == BLE_SUPERVISOR_ID {
                    BluetoothAutoStatus::new(&BLE_SHARED).disable();
                }
            }
        }
        ScheduledAction::ReconcileInterfaces => {
            set_status(usb_status, desired_interfaces & USB_ENABLED != 0);
            if let Some(status) = espnow_status {
                set_status(status, desired_interfaces & ESPNOW_ENABLED != 0);
            }
            #[cfg(feature = "bluetooth-auto")]
            if desired_interfaces & BLUETOOTH_ENABLED != 0 {
                BluetoothAutoStatus::new(&BLE_SHARED).enable();
            } else {
                BluetoothAutoStatus::new(&BLE_SHARED).disable();
            }
        }
        ScheduledAction::SleepSystem => {
            usb_status.disable();
            if let Some(status) = espnow_status {
                status.disable();
            }
            #[cfg(feature = "bluetooth-auto")]
            BluetoothAutoStatus::new(&BLE_SHARED).disable();
            *system_awake = false;
        }
    }
}

fn snapshots(
    usb: &EmbassyInterfaceStatus,
    espnow: Option<&EmbassyInterfaceStatus>,
) -> Result<heapless::Vec<InterfaceSnapshot, INTERFACE_CAPACITY>, RemoteControlHostCommandError> {
    #[cfg(feature = "bluetooth-auto")]
    let bluetooth = BluetoothAutoStatus::new(&BLE_SHARED);
    let mut entries: heapless::Vec<(&dyn InterfaceStatus, Membership), INTERFACE_CAPACITY> =
        heapless::Vec::new();
    entries
        .push((usb, Membership::Independent))
        .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
    if let Some(espnow) = espnow {
        entries
            .push((espnow, Membership::Independent))
            .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
    }
    #[cfg(feature = "bluetooth-auto")]
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
