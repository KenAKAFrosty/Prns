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
    RemoteControlApplyOutcome, RemoteControlCapabilities, RemoteControlDisplayAutoOff,
    RemoteControlDisplayVisibility, RemoteControlInterfacePower, RemoteControlLoRaOutcome,
    RemoteControlPowerOutcome, RemoteControlRequestKind, RemoteControlRequestSet,
    RemoteControlSystemPower,
};
use personal_rns::runtime::{
    RemoteControlHostCommand, RemoteControlHostCommandError, RemoteControlHostResponse,
};

#[cfg(feature = "board-t096")]
use crate::boards::selected as board;
use crate::immediate_display::{ImmediateDisplayDevice, ImmediateDisplayRuntime};

use super::bluetooth::{BLE_SHARED, BLE_SUPERVISOR_ID};

const RESPONSE_GRACE_PERIOD: Duration = Duration::from_millis(250);

type ConfigurationStore = hopspot::SubGConfigurationStore<super::super::learned_state::BoardFlash>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScheduledAction {
    DisableInterface(InterfaceId),
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
    pub system_awake: &'a mut bool,
    pub scheduled_effect: &'a mut Option<ScheduledEffect>,
    pub lora_controller: &'a mut personal_rns::lora::LoRaController<'static>,
    pub subg_store: &'a mut ConfigurationStore,
    pub subg_configuration: &'a mut SubGConfigurationState,
    #[cfg(feature = "board-t096")]
    pub gnss_wanted: &'a mut bool,
}

pub(super) fn capabilities() -> RemoteControlCapabilities {
    let mut requests = RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
    for kind in [
        RemoteControlRequestKind::InventoryInterfaces,
        RemoteControlRequestKind::SetInterfacePower,
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
        assert!(requests.insert(kind), "Remote Control capability is unique");
    }
    #[cfg(feature = "board-t096")]
    assert!(
        requests.insert(RemoteControlRequestKind::SetGnssPower),
        "Remote Control GNSS capability is unique"
    );
    RemoteControlCapabilities::from_requests(requests)
        .expect("the immediate-display nRF capability set includes Describe")
}

pub(super) async fn execute<D: ImmediateDisplayDevice>(
    context: Context<'_, D>,
    command: RemoteControlHostCommand,
) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError> {
    match command {
        RemoteControlHostCommand::InventoryInterfaces { page } => {
            Ok(RemoteControlHostResponse::InventoryInterfaces(
                hopspot::remote_control_inventory_from_snapshots(context.snapshots, page),
            ))
        }
        RemoteControlHostCommand::InventoryInterfacePeers { id, page } => {
            Ok(RemoteControlHostResponse::InventoryInterfacePeers(
                hopspot::remote_control_interface_peers_from_snapshots(context.snapshots, id, page),
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
            let outcome = hopspot::remote_control_interface_config_from_snapshots(
                context.snapshots,
                id,
                |snapshot, card| {
                    hopspot::decorate_hopspot_remote_control_card(
                        snapshot, card, None, profile, None, None,
                    );
                },
            );
            Ok(RemoteControlHostResponse::InventoryInterfaceConfig(outcome))
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
            context.lora_status.enable();
            Ok(RemoteControlHostResponse::SetInterfaceLoRaProfile(
                RemoteControlLoRaOutcome::Applied,
            ))
        }
        RemoteControlHostCommand::DescribeBuild => Ok(RemoteControlHostResponse::DescribeBuild(
            hopspot::hopspot_remote_control_build_version(),
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
                    .await
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
                context.lora_status.enable();
                context.usb_status.enable();
                BluetoothAutoStatus::new(&BLE_SHARED).enable();
                #[cfg(feature = "board-t096")]
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
        RemoteControlHostCommand::SetDisplayVisibility { visibility } => {
            if context.display.visibility() == hopspot::display::DisplayVisibility::Unavailable {
                return Err(RemoteControlHostCommandError::Unsupported);
            }
            let desired_visible = visibility == RemoteControlDisplayVisibility::Visible;
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
            let currently_enabled =
                !matches!(board::gnss_snapshot(), hopspot::GnssSnapshot::Disabled);
            *context.gnss_wanted = desired;
            let outcome = if desired == currently_enabled {
                RemoteControlApplyOutcome::Unchanged
            } else {
                board::control_gnss(if desired {
                    hopspot::GnssReceiverCommand::Enable
                } else {
                    hopspot::GnssReceiverCommand::Disable
                });
                RemoteControlApplyOutcome::Applied
            };
            Ok(RemoteControlHostResponse::SetGnssPower(outcome))
        }
        _ => Err(RemoteControlHostCommandError::Unsupported),
    }
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

fn interface_enabled<D: ImmediateDisplayDevice>(
    context: &Context<'_, D>,
    id: InterfaceId,
) -> Option<bool> {
    if context.lora_status.id() == id {
        Some(context.lora_status.is_enabled())
    } else if context.usb_status.id() == id {
        Some(context.usb_status.is_enabled())
    } else if id == BLE_SUPERVISOR_ID {
        Some(BluetoothAutoStatus::new(&BLE_SHARED).is_enabled())
    } else {
        None
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
    system_awake: &mut bool,
) {
    let Some(effect) = scheduled.as_ref() else {
        return;
    };
    if Instant::now() < effect.apply_at {
        return;
    }
    let effect = scheduled.take().expect("the scheduled effect is present");
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
            *system_awake = false;
        }
    }
}

pub(super) async fn apply_subg_configuration(
    controller: &mut personal_rns::lora::LoRaController<'static>,
    store: &mut ConfigurationStore,
    active: &mut SubGConfigurationState,
    requested: SubGConfigurationState,
) -> Result<(), RemoteControlHostCommandError> {
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
            Err(RemoteControlHostCommandError::PersistenceFailed)
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
