use crate::ControllerInstallation;
use alloc::sync::Arc;
use core::future::Future;
use personal_rns::manifold::tokio::TokioClock;
use personal_rns::prelude::*;
use personal_rns::runtime::{AttachedInterface, NodeRunError};
use tokio::sync::Notify;

#[cfg(test)]
mod behavior;
#[cfg(test)]
mod tests;

pub struct NativeHubRuntime<Run> {
    pub devices: crate::DeviceStore,
    pub handle: PrnsNodeHandle,
    pub clock: TokioClock,
    pub identities: RemoteControlNodeIdentities,
    pub identity_origins: RemoteControlNodeIdentityOrigins,
    pub usb_interface: AttachedInterface,
    pub usb_rescan: Arc<Notify>,
    pub run: Run,
}

pub fn prepare_native_hub(
    installation: ControllerInstallation,
    on_event: impl FnMut(PrnsEvent<'_>, &NoRemoteControlHostControls),
    shutdown: impl Future<Output = ()>,
) -> NativeHubRuntime<impl Future<Output = Result<(), NodeRunError>>> {
    let usb = AutoUsb::default();
    let rescan = usb.rescan_signal();
    prepare_hub(
        installation,
        move |handle| handle.attach(usb),
        rescan,
        on_event,
        shutdown,
    )
}

pub(super) fn prepare_hub(
    installation: ControllerInstallation,
    attach_usb: impl FnOnce(&PrnsNodeHandle) -> AttachedInterface,
    usb_rescan: Arc<Notify>,
    on_event: impl FnMut(PrnsEvent<'_>, &NoRemoteControlHostControls),
    shutdown: impl Future<Output = ()>,
) -> NativeHubRuntime<impl Future<Output = Result<(), NodeRunError>>> {
    let ControllerInstallation {
        state_lock,
        devices,
        identity,
        persistence,
    } = installation;
    let (secrets, identity_origins) = identity.into_parts();
    let identities = secrets.identities();
    let remote_control = RemoteControlService::new(
        secrets,
        RemoteControlInitialControllerGrants::Nobody,
        RemoteControlSelfAnnouncement::Unavailable,
    );
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: remote_control.into(),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: NoRemoteControlHostControls,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event,
        interfaces: ManuallyAttached,
        persistence,
    });
    let handle = node.handle();
    let usb_interface = attach_usb(&handle);
    NativeHubRuntime {
        devices,
        clock: node.clock(),
        handle,
        identities,
        identity_origins,
        usb_interface,
        usb_rescan,
        run: async move {
            let _state_lock = state_lock;
            node.run_until(shutdown).await
        },
    }
}
