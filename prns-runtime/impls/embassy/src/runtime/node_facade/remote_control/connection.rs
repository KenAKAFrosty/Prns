use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::engine::{EstablishLinkFailure, IdentifyFailure};
use crate::identity::IdentityHash;
use crate::routing::links::LinkId;
use crate::runtime::{
    CloseRemoteControlTargetOutcome, ConnectRemoteControlTargetError, RemoteControlAnnounceSelf,
    RemoteControlDescribe, RemoteControlTargetConnection, RemoteControlTargetConnectionControl,
    RemoteControlTargetConnectionTransport, RemoteControlTargetOperationError, SendError,
};
use crate::units::RttMillis;
use crate::wire::DestinationHash;
use prns_core::remote_control::RemoteControlDescription;

use super::{PrnsNodeHandle, RemoteControlHandle};

pub struct RemoteControlTargetHandle<
    'a,
    M: RawMutex,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
> {
    remote_control:
        RemoteControlHandle<'a, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    connection: RemoteControlTargetConnection,
}

impl<
        'a,
        M: RawMutex + Sync,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > PrnsNodeHandle<'a, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    pub async fn connect_remote_control_target(
        &self,
        target: IdentityHash,
    ) -> Result<
        RemoteControlTargetHandle<
            'a,
            M,
            COMMANDS,
            COMPLETIONS,
            REQUEST_COMPLETIONS,
            RESPONSE_BYTES,
        >,
        ConnectRemoteControlTargetError,
    > {
        let connection = self.establish_remote_control_target(target).await?;
        Ok(RemoteControlTargetHandle {
            remote_control: self.remote_control(connection.link_id()),
            connection,
        })
    }
}

impl<
        M: RawMutex + Sync,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > RemoteControlTargetConnectionTransport
    for PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    async fn establish_remote_control_link(
        &self,
        destination: DestinationHash,
    ) -> Result<LinkId, SendError<EstablishLinkFailure>> {
        self.establish_link(destination).await
    }

    async fn identify_remote_control_link(
        &self,
        link_id: LinkId,
        identity: IdentityHash,
    ) -> Result<(), SendError<IdentifyFailure>> {
        self.identify(link_id, identity).await
    }

    fn close_remote_control_link(&self, link_id: LinkId) -> CloseRemoteControlTargetOutcome {
        match self.close_link(link_id) {
            true => CloseRemoteControlTargetOutcome::Queued,
            false => CloseRemoteControlTargetOutcome::NotQueued,
        }
    }
}

impl<
        M: RawMutex + Sync,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > RemoteControlTargetHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    #[must_use]
    pub const fn connection(&self) -> &RemoteControlTargetConnection {
        &self.connection
    }

    pub fn close(self) -> CloseRemoteControlTargetOutcome {
        self.remote_control
            .node
            .close_remote_control_link(self.connection.link_id())
    }

    pub async fn announce_self(&self) -> Result<RttMillis, RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlAnnounceSelf::REQUEST.kind())?;
        self.remote_control
            .announce_self()
            .await
            .map_err(Into::into)
    }

    pub async fn describe(
        &self,
    ) -> Result<(RemoteControlDescription, RttMillis), RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlDescribe::REQUEST.kind())?;
        self.remote_control.describe().await.map_err(Into::into)
    }
}
