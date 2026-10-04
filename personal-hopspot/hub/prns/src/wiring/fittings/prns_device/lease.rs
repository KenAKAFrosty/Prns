use super::PrnsDeviceTransport;
use alloc::sync::Arc;
use personal_rns::runtime::{CloseRemoteControlTargetOutcome, RemoteControlTargetConnection};

enum LinkRelease {
    OnDrop,
    AlreadyReleased,
}

pub(super) struct PrnsLinkLease<Backend: PrnsDeviceTransport> {
    pub(super) remote: RemoteControlTargetConnection,
    backend: Arc<Backend>,
    release: LinkRelease,
}

impl<Backend: PrnsDeviceTransport> PrnsLinkLease<Backend> {
    pub(super) fn new(backend: Arc<Backend>, remote: RemoteControlTargetConnection) -> Self {
        Self {
            remote,
            backend,
            release: LinkRelease::OnDrop,
        }
    }

    pub(super) fn close(mut self) -> CloseRemoteControlTargetOutcome {
        self.release = LinkRelease::AlreadyReleased;
        self.backend
            .close_remote_control_link(self.remote.link_id())
    }
}

impl<Backend: PrnsDeviceTransport> Drop for PrnsLinkLease<Backend> {
    fn drop(&mut self) {
        match self.release {
            LinkRelease::OnDrop => {
                self.backend
                    .close_remote_control_link(self.remote.link_id());
            }
            LinkRelease::AlreadyReleased => {}
        }
    }
}
