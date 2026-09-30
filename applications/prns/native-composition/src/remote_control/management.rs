//! Bounded, typed management over the public authorized target handle.
//!
//! The lifecycle actor owns admission, cancellation and durable-in-process write
//! status. This module owns a single connected exchange and never retries writes.
mod projection;
mod validation;

use std::sync::atomic::{AtomicBool, Ordering};

use personal_rns::prelude::{
    PrnsNodeHandle, RemoteControlError, RemoteControlTargetAccessControl,
    RemoteControlTargetHandle, RemoteControlTargetOperationError, SendError,
};
use personal_rns::remote_control as core;
use tokio::time::Instant;

use super::{connect_reachable_target, identity_hash, CloseTargetLink};
use crate::contract::*;
use projection::*;
use validation::{prepare_change, prepare_query, PreparedChange, PreparedQuery};

type Failure = (RemoteManagementFailureStage, String);

pub fn validate_change_input(input: &ChangeRemoteNodeInput) -> Result<(), String> {
    if identity_hash(&input.target_identity_fingerprint).is_none() {
        return Err("Choose a valid paired node.".to_owned());
    }
    prepare_change(&input.change).map(|_| ())
}

pub async fn read(
    handle: &PrnsNodeHandle,
    input: ReadRemoteNodeInput,
    deadline: Instant,
) -> ReadRemoteNodeOutcome {
    match tokio::time::timeout_at(deadline, read_inner(handle, input, deadline)).await {
        Ok(Ok((available_requests, data, rtt_millis))) => ReadRemoteNodeOutcome::Read {
            available_requests,
            data,
            rtt_millis,
        },
        Ok(Err((stage, detail))) => ReadRemoteNodeOutcome::Failed { stage, detail },
        Err(_) => ReadRemoteNodeOutcome::Failed {
            stage: RemoteManagementFailureStage::Timeout,
            detail: "The node did not respond in time.".to_owned(),
        },
    }
}

pub(super) async fn connect<'a>(
    handle: &'a PrnsNodeHandle,
    target: &[u8],
    deadline: Instant,
) -> Result<RemoteControlTargetHandle<'a>, Failure> {
    let identity = identity_hash(target).ok_or_else(|| {
        (
            RemoteManagementFailureStage::Input,
            "Choose a valid paired node.".to_owned(),
        )
    })?;
    let resolved = handle
        .resolve_remote_control_target(identity)
        .await
        .map_err(|_| {
            (
                RemoteManagementFailureStage::Inventory,
                "This node is no longer paired.".to_owned(),
            )
        })?;
    connect_reachable_target(
        handle,
        identity,
        resolved.endpoint().destination_hash(),
        deadline,
    )
    .await
    .map_err(|error| {
        let super::AnnounceStatus::Failed { stage } =
            super::announce_reachable_connect_failure(error)
        else {
            return (
                RemoteManagementFailureStage::Link,
                "Could not connect to the node.".to_owned(),
            );
        };
        (
            announce_stage(stage),
            "Could not connect to the node. Check its connection and try again.".to_owned(),
        )
    })
}

async fn read_inner(
    handle: &PrnsNodeHandle,
    input: ReadRemoteNodeInput,
    deadline: Instant,
) -> Result<(Vec<RemoteControlRequestKind>, RemoteNodeData, u64), Failure> {
    let query = prepare_query(input.query)
        .map_err(|detail| (RemoteManagementFailureStage::Input, detail))?;
    let connected = connect(handle, &input.target_identity_fingerprint, deadline).await?;
    let _link = CloseTargetLink {
        handle,
        id: connected.connection().link_id(),
    };
    let (description, rtt) = connected.describe().await.map_err(read_failure)?;
    let available = description.available_requests();
    let data = match query {
        PreparedQuery::Overview => {
            let firmware = if available.supports(core::RemoteControlRequestKind::DescribeBuild) {
                let (build, _) = connected.describe_build().await.map_err(read_failure)?;
                Some(build.as_str().ok_or_else(invalid_response)?.to_owned())
            } else {
                None
            };
            let power = if available.supports(core::RemoteControlRequestKind::DescribePower) {
                Some(project_power(
                    connected.describe_power().await.map_err(read_failure)?.0,
                ))
            } else {
                None
            };
            let interfaces =
                if available.supports(core::RemoteControlRequestKind::InventoryInterfaces) {
                    Some(project_interfaces(
                        connected
                            .inventory_interfaces_page(core::RemoteControlInterfacePage::First)
                            .await
                            .map_err(read_failure)?
                            .0,
                        None,
                    )?)
                } else {
                    None
                };
            RemoteNodeData::Overview {
                overview: RemoteNodeOverview {
                    firmware,
                    power,
                    interfaces,
                },
            }
        }
        PreparedQuery::Interfaces(after) => {
            require(
                available,
                core::RemoteControlRequestKind::InventoryInterfaces,
            )?;
            let page = after.map_or(core::RemoteControlInterfacePage::First, |id| {
                core::RemoteControlInterfacePage::After(core::RemoteControlInterfaceCursor::after(
                    id,
                ))
            });
            RemoteNodeData::Interfaces {
                page: project_interfaces(
                    connected
                        .inventory_interfaces_page(page)
                        .await
                        .map_err(read_failure)?
                        .0,
                    after,
                )?,
            }
        }
        PreparedQuery::Interface(id) => {
            let configuration =
                if available.supports(core::RemoteControlRequestKind::InventoryInterfaceConfig) {
                    project_config(
                        connected
                            .inventory_interface_config(id)
                            .await
                            .map_err(read_failure)?
                            .0,
                    )
                } else {
                    RemoteInterfaceConfiguration::Unavailable
                };
            let discovery_groups = if available
                .supports(core::RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups)
            {
                project_groups(
                    connected
                        .inventory_interface_discovery_groups(id)
                        .await
                        .map_err(read_failure)?
                        .0,
                )
            } else {
                RemoteDiscoveryGroups::Unavailable
            };
            RemoteNodeData::Interface {
                details: RemoteInterfaceDetails {
                    interface_id: id.as_bytes().to_vec(),
                    configuration,
                    discovery_groups,
                },
            }
        }
        PreparedQuery::Peers(id, after) => {
            require(
                available,
                core::RemoteControlRequestKind::InventoryInterfacePeers,
            )?;
            let page = after.map_or(core::RemoteControlPeerPage::First, |cursor| {
                core::RemoteControlPeerPage::After(core::RemoteControlPeerCursor::after(cursor))
            });
            let (peers, _) = connected
                .inventory_interface_peers(id, page)
                .await
                .map_err(read_failure)?;
            RemoteNodeData::Peers {
                page: project_peers(peers, id, after)?,
            }
        }
        PreparedQuery::Controllers(after) => {
            require(
                available,
                core::RemoteControlRequestKind::InventoryControllers,
            )?;
            let page = after.map_or(core::RemoteControlControllerPage::First, |identity| {
                core::RemoteControlControllerPage::After(
                    core::RemoteControlControllerCursor::after(identity),
                )
            });
            RemoteNodeData::Controllers {
                page: project_controllers(
                    connected
                        .inventory_controllers_page(page)
                        .await
                        .map_err(read_failure)?
                        .0,
                    after,
                )?,
            }
        }
    };
    Ok((crate::pairing::request_kinds(available), data, rtt.millis()))
}

pub async fn change(
    handle: &PrnsNodeHandle,
    input: ChangeRemoteNodeInput,
    deadline: Instant,
    dispatched: &AtomicBool,
) -> RemoteChangeStatus {
    match tokio::time::timeout_at(deadline, change_inner(handle, input, deadline, dispatched)).await
    {
        Ok(status) => status,
        Err(_) if dispatched.load(Ordering::Acquire) => RemoteChangeStatus::OutcomeUnknown {
            reason: RemoteControlAnnounceUnknownReason::Timeout,
        },
        Err(_) => failure_status((
            RemoteManagementFailureStage::Timeout,
            "The node did not respond before the change was sent.".to_owned(),
        )),
    }
}

async fn change_inner(
    handle: &PrnsNodeHandle,
    input: ChangeRemoteNodeInput,
    deadline: Instant,
    dispatched: &AtomicBool,
) -> RemoteChangeStatus {
    let prepared = match prepare_change(&input.change) {
        Ok(value) => value,
        Err(detail) => return failure_status((RemoteManagementFailureStage::Input, detail)),
    };
    let connected = match connect(handle, &input.target_identity_fingerprint, deadline).await {
        Ok(value) => value,
        Err(error) => return failure_status(error),
    };
    let _link = CloseTargetLink {
        handle,
        id: connected.connection().link_id(),
    };
    if let PreparedChange::RevokeController(controller) = &prepared {
        let resolved = match handle
            .resolve_remote_control_target(connected.connection().target())
            .await
        {
            Ok(resolved) => resolved,
            Err(_) => {
                return failure_status((
                    RemoteManagementFailureStage::Inventory,
                    "This node is no longer paired.".to_owned(),
                ));
            }
        };
        if *controller == resolved.controller().identity_hash() {
            return failure_status((
                RemoteManagementFailureStage::Permission,
                "This phone cannot remove its own access remotely.".to_owned(),
            ));
        }
    }
    let description = match connected.describe().await {
        Ok((description, _)) => description,
        Err(error) => return failure_status(read_failure(error)),
    };
    if let Err(error) = require(description.available_requests(), prepared.kind()) {
        return failure_status(error);
    }
    // This boundary separates safe preflight failures from ambiguous delivery.
    // The actor also consults it when shutdown interrupts the owned future.
    dispatched.store(true, Ordering::Release);
    match execute(&connected, prepared).await {
        Ok(status) => status,
        Err(error) => mutation_failure(error),
    }
}

async fn execute(
    target: &RemoteControlTargetHandle<'_>,
    change: PreparedChange,
) -> Result<RemoteChangeStatus, RemoteControlTargetOperationError> {
    use core::*;
    Ok(match change {
        PreparedChange::InterfacePower(id, power) => {
            match target.set_interface_power(id, power).await?.0 {
                RemoteControlPowerOutcome::Applied => RemoteChangeStatus::Applied,
                RemoteControlPowerOutcome::Unchanged => RemoteChangeStatus::Unchanged,
                RemoteControlPowerOutcome::Scheduled => RemoteChangeStatus::Scheduled,
                RemoteControlPowerOutcome::UnknownInterface => unknown_interface_status(),
                RemoteControlPowerOutcome::Failed => apply_failed(),
            }
        }
        PreparedChange::InterfaceMode(id, mode) => {
            match target.set_interface_mode(id, mode).await?.0 {
                RemoteControlModeOutcome::Applied => RemoteChangeStatus::Applied,
                RemoteControlModeOutcome::UnknownInterface => unknown_interface_status(),
                RemoteControlModeOutcome::Failed => apply_failed(),
            }
        }
        PreparedChange::InterfaceGroup(id, group) => {
            match target.set_interface_group(id, group).await?.0 {
                RemoteControlGroupOutcome::Applied => RemoteChangeStatus::Applied,
                RemoteControlGroupOutcome::UnknownInterface => unknown_interface_status(),
                RemoteControlGroupOutcome::Failed => apply_failed(),
            }
        }
        PreparedChange::InterfaceLoRa(id, profile) => {
            match target.set_interface_lora_profile(id, profile).await?.0 {
                RemoteControlLoRaOutcome::Applied => RemoteChangeStatus::Applied,
                RemoteControlLoRaOutcome::UnknownInterface => unknown_interface_status(),
                RemoteControlLoRaOutcome::Failed => apply_failed(),
            }
        }
        PreparedChange::DiscoveryGroups(id, groups) => match target
            .replace_interface_discovery_groups(id, groups)
            .await?
            .0
        {
            RemoteControlDiscoveryGroupsReplaceOutcome::Applied => RemoteChangeStatus::Applied,
            RemoteControlDiscoveryGroupsReplaceOutcome::Unchanged => RemoteChangeStatus::Unchanged,
            RemoteControlDiscoveryGroupsReplaceOutcome::UnknownInterface => {
                unknown_interface_status()
            }
            RemoteControlDiscoveryGroupsReplaceOutcome::Unsupported => unsupported_status(),
        },
        PreparedChange::GnssPower(value) => apply_status(target.set_gnss_power(value).await?.0),
        PreparedChange::DisplayVisibility(value) => {
            apply_status(target.set_display_visibility(value).await?.0)
        }
        PreparedChange::DisplayAutoOff(value) => {
            apply_status(target.set_display_auto_off(value).await?.0)
        }
        PreparedChange::SystemPower(value) => apply_status(target.set_system_power(value).await?.0),
        PreparedChange::StationUplink(id, value) => {
            apply_status(target.set_station_uplink(id, value).await?.0)
        }
        PreparedChange::RadioMode(value) => apply_status(target.set_esp_radio_mode(value).await?.0),
        PreparedChange::SleepRadios => sleep_status(target.sleep_radios().await?.0),
        PreparedChange::WakeRadios => sleep_status(target.wake_radios().await?.0),
        PreparedChange::RevokeController(controller) => {
            revoke_status(target.revoke_controller(controller).await?.0)
        }
    })
}

fn revoke_status(outcome: core::RemoteControlRevokeControllerOutcome) -> RemoteChangeStatus {
    match outcome {
        core::RemoteControlRevokeControllerOutcome::Applied => RemoteChangeStatus::Applied,
        core::RemoteControlRevokeControllerOutcome::NotFound => RemoteChangeStatus::Unchanged,
        core::RemoteControlRevokeControllerOutcome::Forbidden => failure_status((
            RemoteManagementFailureStage::Permission,
            "This device's access cannot be removed remotely.".to_owned(),
        )),
        core::RemoteControlRevokeControllerOutcome::Busy => failure_status((
            RemoteManagementFailureStage::Busy,
            "The node is busy. Try again shortly.".to_owned(),
        )),
        core::RemoteControlRevokeControllerOutcome::Failed => failure_status((
            RemoteManagementFailureStage::Request,
            "The node could not remove this device's access.".to_owned(),
        )),
    }
}

fn apply_status(outcome: core::RemoteControlApplyOutcome) -> RemoteChangeStatus {
    match outcome {
        core::RemoteControlApplyOutcome::Applied => RemoteChangeStatus::Applied,
        core::RemoteControlApplyOutcome::Unchanged => RemoteChangeStatus::Unchanged,
        core::RemoteControlApplyOutcome::Scheduled => RemoteChangeStatus::Scheduled,
    }
}

fn sleep_status(outcome: core::RemoteControlSleepOutcome) -> RemoteChangeStatus {
    match outcome {
        core::RemoteControlSleepOutcome::Applied => RemoteChangeStatus::Applied,
        core::RemoteControlSleepOutcome::Unavailable => unsupported_status(),
        core::RemoteControlSleepOutcome::Failed => apply_failed(),
    }
}

fn failure_status((stage, detail): Failure) -> RemoteChangeStatus {
    RemoteChangeStatus::Failed { stage, detail }
}
fn apply_failed() -> RemoteChangeStatus {
    failure_status((
        RemoteManagementFailureStage::Request,
        "The node could not apply the change.".to_owned(),
    ))
}
fn unknown_interface_status() -> RemoteChangeStatus {
    failure_status(unknown_interface())
}
fn unsupported_status() -> RemoteChangeStatus {
    failure_status((
        RemoteManagementFailureStage::Unsupported,
        "This control is not available on the node.".to_owned(),
    ))
}
fn unknown_interface() -> Failure {
    (
        RemoteManagementFailureStage::UnknownInterface,
        "This connection no longer exists. Refresh the node.".to_owned(),
    )
}
fn invalid_response() -> Failure {
    (
        RemoteManagementFailureStage::Response,
        "The node returned an invalid response.".to_owned(),
    )
}

pub(super) fn require(
    available: &core::RemoteControlRequestSet,
    kind: core::RemoteControlRequestKind,
) -> Result<(), Failure> {
    if available.supports(kind) {
        Ok(())
    } else {
        Err((
            RemoteManagementFailureStage::Unsupported,
            "This control is not available with the node's current access.".to_owned(),
        ))
    }
}

fn announce_stage(stage: RemoteControlAnnounceFailureStage) -> RemoteManagementFailureStage {
    match stage {
        RemoteControlAnnounceFailureStage::Input => RemoteManagementFailureStage::Input,
        RemoteControlAnnounceFailureStage::Busy => RemoteManagementFailureStage::Busy,
        RemoteControlAnnounceFailureStage::Inventory => RemoteManagementFailureStage::Inventory,
        RemoteControlAnnounceFailureStage::Route => RemoteManagementFailureStage::Route,
        RemoteControlAnnounceFailureStage::Link => RemoteManagementFailureStage::Link,
        RemoteControlAnnounceFailureStage::Identification => {
            RemoteManagementFailureStage::Identification
        }
        RemoteControlAnnounceFailureStage::Permission => RemoteManagementFailureStage::Permission,
        RemoteControlAnnounceFailureStage::Request => RemoteManagementFailureStage::Request,
        RemoteControlAnnounceFailureStage::Node => RemoteManagementFailureStage::Node,
    }
}

fn protocol_failure(error: core::RemoteControlProtocolError) -> Failure {
    use core::RemoteControlProtocolError as Error;
    let (stage, detail) = match error {
        Error::Busy { .. } => (
            RemoteManagementFailureStage::Busy,
            "The node is busy. Try again shortly.",
        ),
        Error::UnsupportedRequest { .. }
        | Error::UnknownRequestKind { .. }
        | Error::UnsupportedVersion { .. } => (
            RemoteManagementFailureStage::Unsupported,
            "This control is not supported by the node.",
        ),
        Error::PersistenceFailed { .. } => (
            RemoteManagementFailureStage::Persistence,
            "The node could not save the change.",
        ),
        Error::RollbackFailed { .. } => (
            RemoteManagementFailureStage::Rollback,
            "The node could not restore its previous settings. Check the node before making another change.",
        ),
        Error::ApplyFailed { .. } => (
            RemoteManagementFailureStage::Request,
            "The node could not apply the change.",
        ),
        Error::MalformedRequest | Error::InternalFailure { .. } => (
            RemoteManagementFailureStage::Request,
            "The node could not complete the request.",
        ),
    };
    (stage, detail.to_owned())
}

pub(super) fn read_failure(error: RemoteControlTargetOperationError) -> Failure {
    let stage = match error {
        RemoteControlTargetOperationError::NotPermitted(_) => {
            RemoteManagementFailureStage::Permission
        }
        RemoteControlTargetOperationError::Exchange(RemoteControlError::Remote(error)) => {
            return protocol_failure(error);
        }
        RemoteControlTargetOperationError::Exchange(RemoteControlError::Request(
            SendError::Failed(failure),
        )) => match super::classify_send_request_failure(failure) {
            super::SendRequestFailureClass::Timeout => RemoteManagementFailureStage::Timeout,
            super::SendRequestFailureClass::Link => RemoteManagementFailureStage::Link,
            super::SendRequestFailureClass::Request => RemoteManagementFailureStage::Request,
        },
        RemoteControlTargetOperationError::Exchange(RemoteControlError::Request(
            SendError::Busy,
        )) => RemoteManagementFailureStage::Busy,
        RemoteControlTargetOperationError::Exchange(RemoteControlError::Request(
            SendError::NodeStopped,
        )) => RemoteManagementFailureStage::Node,
        RemoteControlTargetOperationError::Exchange(
            RemoteControlError::Response(_) | RemoteControlError::UnexpectedResponse { .. },
        ) => RemoteManagementFailureStage::Response,
        RemoteControlTargetOperationError::Exchange(_) => RemoteManagementFailureStage::Request,
    };
    (
        stage,
        "Could not read the node. Check its connection and try again.".to_owned(),
    )
}

pub(super) fn mutation_failure(error: RemoteControlTargetOperationError) -> RemoteChangeStatus {
    if let RemoteControlTargetOperationError::Exchange(RemoteControlError::Remote(error)) = error {
        return failure_status(protocol_failure(error));
    }
    if matches!(
        error,
        RemoteControlTargetOperationError::Exchange(RemoteControlError::Request(SendError::Busy))
    ) {
        return failure_status((
            RemoteManagementFailureStage::Busy,
            "The node is busy. Try again shortly.".to_owned(),
        ));
    }
    match super::announce_exchange_failure(error) {
        RemoteControlAnnounceStatus::OutcomeUnknown { reason } => {
            RemoteChangeStatus::OutcomeUnknown { reason }
        }
        RemoteControlAnnounceStatus::Failed { stage } => failure_status((
            announce_stage(stage),
            "The change could not be sent to the node.".to_owned(),
        )),
        _ => apply_failed(),
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "host-test"))]
mod tests_wire;
