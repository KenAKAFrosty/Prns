use prns_core::interfaces::weave as weave_core;

use crate::serial::open_host_serial;
use crate::weave::WeaveInterface;

use super::{AttachmentResult, InterfaceConstruction, RECONNECT_POLICY};

pub(super) fn stand_up(construction: InterfaceConstruction<'_>, device: &str) -> AttachmentResult {
    let open_path = device.to_string();
    let weave = WeaveInterface::with_generated_identity(
        move || {
            let open_path = open_path.clone();
            async move { open_host_serial(&open_path, weave_core::WEAVE_SERIAL_BAUD) }
        },
        RECONNECT_POLICY,
        construction.interface.policy,
        device.as_bytes(),
    );
    let weave = weave?;
    let attached = construction.attach(weave);
    Ok(attached.id())
}
