#[cfg(feature = "bluetooth-auto")]
use crate::bluetooth_auto::AutoBle;

#[cfg(feature = "bluetooth-auto")]
use super::{AttachmentResult, InterfaceConstruction};

#[cfg(feature = "bluetooth-auto")]
pub(super) fn stand_up(construction: InterfaceConstruction<'_>) -> AttachmentResult {
    let interface = AutoBle::with_policy(construction.interface.policy);
    let attached = construction.attach(interface);
    Ok(attached.id())
}
