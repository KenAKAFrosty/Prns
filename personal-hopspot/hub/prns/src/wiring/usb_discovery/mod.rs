use crate::{ControllerInstallation, NativeHubRuntime, UsbDiscoveryUpdate};
use alloc::sync::Arc;
use core::future::Future;
use personal_rns::prelude::*;
use personal_rns::runtime::{AttachedInterface, NodeRunError};
use std::sync::OnceLock;
use tokio::sync::Notify;

#[cfg(test)]
mod behavior;
mod handle;
#[cfg(test)]
mod tests;

pub use handle::{UsbDiscoveryFailure, UsbDiscoveryHandle};

pub struct NativeUsbDiscoveryRuntime<const CAPACITY: usize, Run> {
    pub native: NativeHubRuntime<Run>,
    pub discovery: UsbDiscoveryHandle<CAPACITY>,
}

pub enum NativeUsbDiscoveryEvent<'a, const CAPACITY: usize> {
    Discovery(Result<UsbDiscoveryUpdate<CAPACITY>, UsbDiscoveryFailure>),
    Prns(PrnsEvent<'a>),
}

pub fn prepare_native_usb_discovery<const CAPACITY: usize>(
    installation: ControllerInstallation,
    on_event: impl FnMut(NativeUsbDiscoveryEvent<'_, CAPACITY>),
    shutdown: impl Future<Output = ()>,
) -> NativeUsbDiscoveryRuntime<CAPACITY, impl Future<Output = Result<(), NodeRunError>>> {
    let usb = AutoUsb::default();
    let rescan = usb.rescan_signal();
    prepare_discovery(
        installation,
        move |handle| handle.attach(usb),
        rescan,
        on_event,
        shutdown,
    )
}

fn prepare_discovery<const CAPACITY: usize>(
    installation: ControllerInstallation,
    attach_usb: impl FnOnce(&PrnsNodeHandle) -> AttachedInterface,
    rescan: Arc<Notify>,
    mut on_event: impl FnMut(NativeUsbDiscoveryEvent<'_, CAPACITY>),
    shutdown: impl Future<Output = ()>,
) -> NativeUsbDiscoveryRuntime<CAPACITY, impl Future<Output = Result<(), NodeRunError>>> {
    let discovery = Arc::new(OnceLock::<UsbDiscoveryHandle<CAPACITY>>::new());
    let event_discovery = discovery.clone();
    let native = super::runtime::prepare_hub(
        installation,
        attach_usb,
        rescan,
        move |event, _| {
            on_event(event_discovery.wait().receive(event));
        },
        shutdown,
    );
    let discovery = discovery
        .get_or_init(|| UsbDiscoveryHandle::new(native.usb_interface.id(), native.clock.clone()))
        .clone();
    NativeUsbDiscoveryRuntime { native, discovery }
}
