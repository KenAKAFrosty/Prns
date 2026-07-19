mod byte_stream;
mod handle_capabilities;
mod interface_lifecycle;
mod node_lifecycle;
mod persistence;
mod request_response;
mod resource_admission;
mod resource_transfer;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::engine::{
    AnnounceNow, AnnounceNowFailure, CloseLink, CommandId, EngineCommand, EstablishLink,
    EstablishLinkFailure, Identify, IdentifyFailure, IssuedCommand, LinkEstablished,
    PacketReceiptDelivered, PathFound, PathRequestId, RequestPath, RequestPathFailure,
    SendSinglePacket, SendSinglePacketFailure, SendSinglePacketPayload, Settlement,
    PATH_REQUEST_ID_LEN,
};
use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::reactor::driver::HostCommand;
use crate::routing::links::LinkId;
use crate::wire::DestinationHash;

use super::request_router::RespondToken;
use super::{InterfaceStore, SendError};
pub use byte_stream::{ByteStreamReader, ByteStreamWriter, StreamId};
pub use interface_lifecycle::{
    AttachIntent, Attachable, AttachedInterface, AttachedSupervisor, DetachedFleet, Fleet,
    InterfaceAttachmentMetadata, InterfaceSupervisor,
};
use interface_lifecycle::{DriverMsg, RegisteredInterface};
pub use node_lifecycle::{
    NodeRunError, NonRoutingIdentityError, PrnsNode, RegisterRequestRouteError,
    SharedInstanceIdentityError,
};
pub use persistence::{
    boot_timeline_origin, DestinationIdentitySeedReport, FlushError, FlushMark, FlushReport,
    PrepareFlushError, PreparedFlush, RatchetSeedReport, RegionFlush, RouteSeedProgress,
    RouteSeedReport, TunnelSeedReport,
};
pub use resource_admission::{ResourceAdmissionPeer, ResourceOfferAdmission, ResourceOfferMonitor};
pub use resource_transfer::{
    PreparedResourceReceiver, ResourceProgress, ResourceReceipt, ResourceReceiveError,
    ResourceSendError, SegmentCompression, AUTO_COMPRESS_MAX_LEN,
};

/// A cloneable, `Send` handle to a running node: the proactive surface. Every [`CommandId`] is minted from one counter, so a fire-and-forget [`issue`](Self::issue) can never collide with an awaited [`send_single_packet`](Self::send_single_packet) or a runner's respond.
#[derive(Clone)]
pub struct PrnsNodeHandle {
    commands: UnboundedSender<HostCommand>,
    ids: Arc<AtomicU64>,
    notify_tx: UnboundedSender<InterfaceId>,
    iface_build: UnboundedSender<DriverMsg>,
    interfaces: Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    store: InterfaceStore,
    resource_admission: resource_admission::ResourceAdmissionRegistry,
    entropy: crate::reactor::driver::TokioEntropy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPathError {
    EntropyUnavailable,
    Failed(RequestPathFailure),
    NodeStopped,
}

impl PrnsNodeHandle {
    #[cfg(test)]
    pub(crate) fn over(commands: UnboundedSender<HostCommand>) -> Self {
        let (notify_tx, _notify_rx) = tokio::sync::mpsc::unbounded_channel();
        let (iface_build, _iface_build_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            commands,
            ids: Arc::new(AtomicU64::new(0)),
            notify_tx,
            iface_build,
            interfaces: Arc::new(Mutex::new(HashMap::new())),
            store: InterfaceStore::new(),
            resource_admission: resource_admission::ResourceAdmissionRegistry::default(),
            entropy: crate::reactor::driver::TokioEntropy,
        }
    }

    pub fn fill_entropy(&self, bytes: &mut [u8]) {
        self.entropy.fill(bytes);
    }

    fn mint(&self) -> CommandId {
        CommandId(self.ids.fetch_add(1, Ordering::Relaxed))
    }

    #[must_use]
    pub fn interface_store(&self) -> InterfaceStore {
        self.store.clone()
    }

    pub fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        let id = self.mint();
        self.commands
            .send(HostCommand::Engine(IssuedCommand { id, command }))
            .ok()?;
        Some(id)
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "prns.command.send_single_packet",
            level = "debug",
            skip_all,
            fields(bytes = data.len(), destination = ?destination.as_bytes()),
            err(Debug)
        )
    )]
    pub async fn send_single_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>> {
        let payload =
            SendSinglePacketPayload::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        match self
            .settle(EngineCommand::SendSinglePacket(SendSinglePacket {
                destination,
                payload,
            }))
            .await
        {
            Some(Settlement::SendSinglePacket(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    /// Bring a link up to `destination` and await it: `Ok(LinkId)` once the peer's proof validates, or the typed reason it never established. The resolved id is the handle every link-scoped verb takes.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "prns.command.establish_link",
            level = "debug",
            skip_all,
            fields(destination = ?destination.as_bytes()),
            err(Debug)
        )
    )]
    pub async fn establish_link(
        &self,
        destination: DestinationHash,
    ) -> Result<LinkId, SendError<EstablishLinkFailure>> {
        self.establish_link_with_rtt(destination)
            .await
            .map(|established| established.link_id)
    }

    pub async fn establish_link_with_rtt(
        &self,
        destination: DestinationHash,
    ) -> Result<LinkEstablished, SendError<EstablishLinkFailure>> {
        match self
            .settle(EngineCommand::EstablishLink(EstablishLink { destination }))
            .await
        {
            Some(Settlement::EstablishLink(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    pub async fn request_path(
        &self,
        destination: DestinationHash,
    ) -> Result<PathFound, RequestPathError> {
        let mut request_id = [0; PATH_REQUEST_ID_LEN];
        getrandom::getrandom(&mut request_id).map_err(|_| RequestPathError::EntropyUnavailable)?;
        match self
            .settle(EngineCommand::RequestPath(RequestPath {
                destination,
                id: PathRequestId::new(request_id),
            }))
            .await
        {
            Some(Settlement::RequestPath(result)) => result.map_err(RequestPathError::Failed),
            Some(_) | None => Err(RequestPathError::NodeStopped),
        }
    }

    pub async fn identify(
        &self,
        link_id: LinkId,
        identity: IdentityHash,
    ) -> Result<(), SendError<IdentifyFailure>> {
        match self
            .settle(EngineCommand::Identify(Identify { link_id, identity }))
            .await
        {
            Some(Settlement::Identify(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    pub async fn announce_now(
        &self,
        announce: AnnounceNow,
    ) -> Result<(), SendError<AnnounceNowFailure>> {
        match self.settle(EngineCommand::AnnounceNow(announce)).await {
            Some(Settlement::AnnounceNow(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    pub(crate) async fn settle(&self, command: EngineCommand) -> Option<Settlement> {
        let id = self.mint();
        let (completion, settled) = oneshot::channel();
        self.commands
            .send(HostCommand::AwaitedEngine {
                issued: IssuedCommand { id, command },
                completion,
            })
            .ok()?;
        settled.await.ok()
    }

    pub fn close_link(&self, link_id: LinkId) -> bool {
        self.issue(EngineCommand::CloseLink(CloseLink { link_id }))
            .is_some()
    }
}

impl super::PrnsNodeApi for PrnsNodeHandle {
    fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        self.issue(command)
    }

    async fn send_single_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>> {
        self.send_single_packet(destination, data).await
    }

    fn respond(&self, responder: RespondToken, body: &[u8]) -> bool {
        self.respond(responder, body).is_some()
    }

    fn close_link(&self, link_id: LinkId) -> bool {
        self.close_link(link_id)
    }
}

#[cfg(test)]
mod tests;
