#[cfg(feature = "bluetooth-auto")]
use crate::bluetooth_auto::AutoBle;

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
    let interface = AutoBle::with_policy(identity, construction.interface.policy);
    let attached = construction.attach(interface);
    Ok(attached.id())
}
