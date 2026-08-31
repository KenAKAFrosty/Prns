#[cfg(feature = "bluetooth-auto")]
use crate::bluetooth_auto::AutoBle;
#[cfg(feature = "bluetooth-auto")]
use prns_config::PlannedMedium;
#[cfg(feature = "bluetooth-auto")]
use prns_runtime::interfaces::bluetooth_auto::group_tag;

#[cfg(feature = "bluetooth-auto")]
use super::{AttachmentResult, InterfaceConstruction, PlanFailure, PlanRuntimeContext};

#[cfg(feature = "bluetooth-auto")]
pub(super) fn stand_up(
    construction: InterfaceConstruction<'_>,
    context: &PlanRuntimeContext,
) -> AttachmentResult {
    let identity = context
        .ble_identity
        .ok_or(PlanFailure::MissingBleIdentity)?;
    let group_id = match &construction.interface.medium {
        PlannedMedium::PrnsBluetoothAuto { group_id } => group_id.as_bytes(),
        _ => b"reticulum",
    };
    let interface = AutoBle::with_policy_and_group(
        identity,
        construction.interface.policy,
        group_tag(group_id),
    );
    let attached = construction.attach(interface);
    Ok(attached.id())
}
