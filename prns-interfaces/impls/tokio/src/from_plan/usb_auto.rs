#[cfg(feature = "usb")]
use crate::usb_auto::AutoUsb;

#[cfg(feature = "usb")]
use super::{AttachmentResult, InterfaceConstruction};

#[cfg(feature = "usb")]
pub(super) fn stand_up(construction: InterfaceConstruction<'_>) -> AttachmentResult {
    let interface = AutoUsb::default().with_policy(construction.interface.policy);
    let attached = construction.attach(interface);
    Ok(attached.id())
}
