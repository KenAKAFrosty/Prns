mod byte_stream;
mod handle_capabilities;
mod interface_lifecycle;
mod node_lifecycle;
mod persistence;
mod remote_control;
mod request_response;
mod resource_admission;
mod resource_transfer;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::engine::{
    AllowRequester, AllowRequesterFailure, AnnounceNow, CloseLink, CommandId, EstablishLink,
    EstablishLinkFailure, Identify, IdentifyFailure, IssuedCommand, LinkEstablished,
    PacketReceiptDelivered, PathFound, PathRequestId, PrnsCommand, RequestPath, RequestPathFailure,
    SendGroup, SendGroupFailure, SendGroupPayload, SendPlainPacket, SendPlainPacketFailure,
    SendPlainPacketPayload, SendSinglePacket, SendSinglePacketFailure, SendSinglePacketPayload,
    SendToChannel, SendToChannelBody, SendToChannelFailure, SendToLink, SendToLinkFailure,
    SendToLinkPayload, SetRegisteredAnnounceAppData, Settlement, PATH_REQUEST_ID_LEN,
};
use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::manifold::driver::HostCommand;
use crate::routing::links::channel::MessageType;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::{RequestPathHash, RequestPolicy};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;

use super::remote_control_access::RemoteControlAccessSender;
#[cfg(test)]
use super::remote_control_access::{remote_control_access_lane, RemoteControlAccessReceiver};
use super::request_endpoints::RespondToken;
use super::{InterfaceStore, SendError};
pub use byte_stream::{ByteStreamReader, ByteStreamWriter, StreamId};
pub use interface_lifecycle::{
    AttachIntent, Attachable, AttachedInterface, AttachedSupervisor, DetachedFleet, Fleet,
    InterfaceAttachmentMetadata, InterfaceSupervisor,
};
use interface_lifecycle::{DriverMsg, RegisteredInterface};
pub use node_lifecycle::{
    NodeRunError, NonRoutingIdentityError, PrnsNode, RegisterRequestEndpointError,
    SharedInstanceIdentityError,
};
pub use persistence::{
    boot_timeline_origin, wall_clock_timeline_origin, DefaultLocationError,
    DestinationIdentitySeedReport, FlushError, FlushFailurePolicy, FlushMark, FlushReport,
    NodePersistence, PersistenceEvent, PersistenceFlushStatus, PersistenceIntent,
    PersistenceRestoreReport, PersistenceTrigger, PersistenceWorker, PrepareFlushError,
    PreparedFlush, RatchetSeedReport, RegionFlush, RouteSeedProgress, RouteSeedReport, SaveOnLearn,
    SaveOnLearnWiring, TunnelSeedReport,
};
pub use remote_control::RemoteControlHandle;
pub use request_response::{RequestOptions, ResponseSendError};
pub use resource_admission::{ResourceAdmissionPeer, ResourceOfferAdmission, ResourceOfferMonitor};
pub use resource_transfer::{
    PreparedResourceReceiver, ResourceProgress, ResourceReceipt, ResourceReceiveError,
    ResourceSendError, SegmentCompression, AUTO_COMPRESS_MAX_LEN,
};

#[cfg(test)]
pub(crate) fn test_remote_control_service(
) -> prns_core::remote_control::RemoteControlService<'static> {
    use prns_core::identity::vault::IdentitySecretKey;
    use prns_core::remote_control::{
        RemoteControlControllerIdentitySecret, RemoteControlInitialAccess,
        RemoteControlNodeIdentitySecrets, RemoteControlPublicAppData,
        RemoteControlSelfAnnouncement, RemoteControlService, RemoteControlTargetIdentitySecret,
    };

    let identity_secrets = RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
            [0x71; crate::identity::IDENTITY_SECRET_KEY_LEN],
        )),
        RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
            [0x72; crate::identity::IDENTITY_SECRET_KEY_LEN],
        )),
    )
    .expect("distinct test identities");
    RemoteControlService::new(
        identity_secrets,
        RemoteControlPublicAppData::try_from(b"".as_slice()).expect("empty app data"),
        RemoteControlInitialAccess::Nobody,
        RemoteControlSelfAnnouncement::Unavailable,
    )
}

#[cfg(test)]
pub(crate) fn test_remote_control_grant(
    request: prns_core::remote_control::RemoteControlRequestKind,
) -> prns_core::remote_control::RemoteControlControllerGrant {
    let identities = test_remote_control_service()
        .configuration()
        .unwrap()
        .identity_secrets()
        .identities();
    prns_core::remote_control::RemoteControlControllerGrant::new(
        *identities.controller(),
        prns_core::remote_control::RemoteControlRequestSet::only(request),
    )
    .unwrap()
}

/// A cloneable, `Send` handle to a running node: the proactive surface. Every [`CommandId`] is minted from one counter, so a fire-and-forget [`issue`](Self::issue) can never collide with an awaited [`send_single_packet`](Self::send_single_packet) or a runner's respond.
#[derive(Clone)]
pub struct PrnsNodeHandle {
    commands: UnboundedSender<HostCommand>,
    ids: Arc<AtomicU64>,
    attachment_epochs: Arc<AtomicU64>,
    notify_tx: UnboundedSender<InterfaceId>,
    iface_build: UnboundedSender<DriverMsg>,
    interfaces: Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    store: InterfaceStore,
    resource_admission: resource_admission::ResourceAdmissionRegistry,
    entropy: crate::manifold::driver::TokioEntropy,
    timing_oracle: Arc<Mutex<Option<Arc<dyn BitrateTimingOracle>>>>,
    pub(super) remote_control_access: RemoteControlAccessSender,
}

/// An optional shared-instance timing source. Directly attached nodes do not need one;
/// clients install the daemon-backed implementation when joining the local bus.
pub trait BitrateTimingOracle: Send + Sync {
    fn first_hop_timeout(
        &self,
        destination: DestinationHash,
    ) -> Pin<Box<dyn Future<Output = Option<Duration>> + Send + '_>>;

    fn medium_path_timeout(&self) -> Pin<Box<dyn Future<Output = Option<Duration>> + Send + '_>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPathError {
    EntropyUnavailable,
    Failed(RequestPathFailure),
    NodeStopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeRequestHandlerError {
    TableFull,
    NodeStopped,
}

impl core::fmt::Display for RuntimeRequestHandlerError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TableFull => formatter.write_str("the request-handler table is full"),
            Self::NodeStopped => formatter.write_str("the node stopped before applying the route"),
        }
    }
}

impl std::error::Error for RuntimeRequestHandlerError {}

impl PrnsNodeHandle {
    #[cfg(test)]
    pub(crate) fn over(commands: UnboundedSender<HostCommand>) -> Self {
        Self::over_with_remote_control_access(commands).0
    }

    #[cfg(test)]
    pub(super) fn over_with_remote_control_access(
        commands: UnboundedSender<HostCommand>,
    ) -> (Self, RemoteControlAccessReceiver) {
        let (notify_tx, _notify_rx) = tokio::sync::mpsc::unbounded_channel();
        let (iface_build, _iface_build_rx) = tokio::sync::mpsc::unbounded_channel();
        let (remote_control_access, remote_control_access_rx) = remote_control_access_lane();
        (
            Self {
                commands,
                ids: Arc::new(AtomicU64::new(0)),
                attachment_epochs: Arc::new(AtomicU64::new(0)),
                notify_tx,
                iface_build,
                interfaces: Arc::new(Mutex::new(HashMap::new())),
                store: InterfaceStore::new(),
                resource_admission: resource_admission::ResourceAdmissionRegistry::default(),
                entropy: crate::manifold::driver::TokioEntropy,
                timing_oracle: Arc::new(Mutex::new(None)),
                remote_control_access,
            },
            remote_control_access_rx,
        )
    }

    pub fn fill_entropy(&self, bytes: &mut [u8]) {
        self.entropy.fill(bytes);
    }

    /// Installs a daemon-backed timing source for subsequent high-level network operations.
    pub fn install_bitrate_timing_oracle(&self, oracle: Arc<dyn BitrateTimingOracle>) {
        if let Ok(mut installed) = self.timing_oracle.lock() {
            *installed = Some(oracle);
        }
    }

    async fn first_hop_command_timing(
        &self,
        destination: DestinationHash,
    ) -> crate::engine::CommandTiming {
        let oracle = self
            .timing_oracle
            .lock()
            .ok()
            .and_then(|installed| installed.clone());
        let first_hop_timeout_floor_ms = match oracle {
            Some(oracle) => oracle
                .first_hop_timeout(destination)
                .await
                .map(duration_millis_saturating),
            None => None,
        };
        crate::engine::CommandTiming {
            first_hop_timeout_floor_ms,
            path_timeout_floor_ms: None,
        }
    }

    async fn path_command_timing(&self) -> crate::engine::CommandTiming {
        let oracle = self
            .timing_oracle
            .lock()
            .ok()
            .and_then(|installed| installed.clone());
        let path_timeout_floor_ms = match oracle {
            Some(oracle) => oracle
                .medium_path_timeout()
                .await
                .map(duration_millis_saturating),
            None => None,
        };
        crate::engine::CommandTiming {
            first_hop_timeout_floor_ms: None,
            path_timeout_floor_ms,
        }
    }

    fn mint(&self) -> CommandId {
        CommandId(self.ids.fetch_add(1, Ordering::Relaxed))
    }

    #[must_use]
    pub fn interface_store(&self) -> InterfaceStore {
        self.store.clone()
    }

    pub fn issue(&self, command: PrnsCommand) -> Option<CommandId> {
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
        let timing = self.first_hop_command_timing(destination).await;
        match self
            .settle_with_timing(
                PrnsCommand::SendSinglePacket(SendSinglePacket {
                    destination,
                    payload,
                }),
                timing,
            )
            .await
        {
            Some(Settlement::SendSinglePacket(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    pub async fn send_plain_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<(), SendError<SendPlainPacketFailure>> {
        let payload =
            SendPlainPacketPayload::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        match self
            .settle(PrnsCommand::SendPlainPacket(SendPlainPacket {
                destination,
                payload,
            }))
            .await
        {
            Some(Settlement::SendPlainPacket(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    pub async fn send_group_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<(), SendError<SendGroupFailure>> {
        let payload =
            SendGroupPayload::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        match self
            .settle(PrnsCommand::SendGroup(SendGroup {
                destination,
                payload,
            }))
            .await
        {
            Some(Settlement::SendGroup(result)) => result.map_err(SendError::Failed),
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
        let timing = self.first_hop_command_timing(destination).await;
        match self
            .settle_with_timing(
                PrnsCommand::EstablishLink(EstablishLink { destination }),
                timing,
            )
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
        let timing = self.path_command_timing().await;
        match self
            .settle_with_timing(
                PrnsCommand::RequestPath(RequestPath {
                    destination,
                    id: PathRequestId::new(request_id),
                }),
                timing,
            )
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
            .settle(PrnsCommand::Identify(Identify { link_id, identity }))
            .await
        {
            Some(Settlement::Identify(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    pub async fn send_link_packet(
        &self,
        link_id: LinkId,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendToLinkFailure>> {
        let payload =
            SendToLinkPayload::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        match self
            .settle(PrnsCommand::SendToLink(SendToLink { link_id, payload }))
            .await
        {
            Some(Settlement::SendToLink(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    pub async fn send_channel_message(
        &self,
        link_id: LinkId,
        message_type: MessageType,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendToChannelFailure>> {
        let body = SendToChannelBody::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        match self
            .settle(PrnsCommand::SendToChannel(SendToChannel {
                link_id,
                message_type,
                body,
            }))
            .await
        {
            Some(Settlement::SendToChannel(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    pub async fn announce_now(&self, announce: AnnounceNow) -> Result<(), super::AnnounceNowError> {
        match self.settle(PrnsCommand::AnnounceNow(announce)).await {
            Some(Settlement::AnnounceNow(Ok(()))) => Ok(()),
            Some(Settlement::AnnounceNow(Err(failure))) => {
                Err(super::AnnounceNowError::from_failure(failure))
            }
            Some(_) | None => Err(super::AnnounceNowError::NodeStopped),
        }
    }

    pub async fn set_registered_announce_app_data(
        &self,
        set: SetRegisteredAnnounceAppData,
    ) -> Result<(), super::SetRegisteredAnnounceAppDataError> {
        match self
            .settle(PrnsCommand::SetRegisteredAnnounceAppData(set))
            .await
        {
            Some(Settlement::SetRegisteredAnnounceAppData(Ok(()))) => Ok(()),
            Some(Settlement::SetRegisteredAnnounceAppData(Err(failure))) => Err(
                super::SetRegisteredAnnounceAppDataError::from_failure(failure),
            ),
            Some(_) | None => Err(super::SetRegisteredAnnounceAppDataError::NodeStopped),
        }
    }

    pub async fn allow_requester(
        &self,
        allow: AllowRequester,
    ) -> Result<(), SendError<AllowRequesterFailure>> {
        match self.settle(PrnsCommand::AllowRequester(allow)).await {
            Some(Settlement::AllowRequester(result)) => result.map_err(SendError::Failed),
            Some(_) | None => Err(SendError::NodeStopped),
        }
    }

    /// Register or replace a request path while the node is running. This resolves only after the manifold has applied the mutation.
    pub async fn register_request_path(
        &self,
        destination: DestinationHash,
        path: &str,
        policy: RequestPolicy,
    ) -> Result<(), RuntimeRequestHandlerError> {
        let (ready, applied) = oneshot::channel();
        self.commands
            .send(HostCommand::RegisterRequestHandler {
                destination,
                path_hash: RequestPathHash::of(path),
                policy,
                ready,
            })
            .map_err(|_| RuntimeRequestHandlerError::NodeStopped)?;
        match applied.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(TablePushError::TableFull)) => Err(RuntimeRequestHandlerError::TableFull),
            Err(_) => Err(RuntimeRequestHandlerError::NodeStopped),
        }
    }

    /// Remove a request path while the node is running. `Ok(false)` means the route was already absent.
    pub async fn unregister_request_path(
        &self,
        destination: DestinationHash,
        path: &str,
    ) -> Result<bool, RuntimeRequestHandlerError> {
        let (ready, applied) = oneshot::channel();
        self.commands
            .send(HostCommand::UnregisterRequestHandler {
                destination,
                path_hash: RequestPathHash::of(path),
                ready,
            })
            .map_err(|_| RuntimeRequestHandlerError::NodeStopped)?;
        applied
            .await
            .map_err(|_| RuntimeRequestHandlerError::NodeStopped)
    }

    pub(crate) async fn settle(&self, command: PrnsCommand) -> Option<Settlement> {
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

    async fn settle_with_timing(
        &self,
        command: PrnsCommand,
        timing: crate::engine::CommandTiming,
    ) -> Option<Settlement> {
        if timing == crate::engine::CommandTiming::default() {
            return self.settle(command).await;
        }
        let id = self.mint();
        let (completion, settled) = oneshot::channel();
        self.commands
            .send(HostCommand::AwaitedEngineWithTiming {
                issued: IssuedCommand { id, command },
                timing,
                completion,
            })
            .ok()?;
        settled.await.ok()
    }

    pub fn close_link(&self, link_id: LinkId) -> bool {
        self.issue(PrnsCommand::CloseLink(CloseLink { link_id }))
            .is_some()
    }
}

fn duration_millis_saturating(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

impl super::PrnsNodeApi for PrnsNodeHandle {
    fn issue(&self, command: PrnsCommand) -> Option<CommandId> {
        self.issue(command)
    }

    async fn announce_now(&self, announce: AnnounceNow) -> Result<(), super::AnnounceNowError> {
        self.announce_now(announce).await
    }

    async fn set_registered_announce_app_data(
        &self,
        set: SetRegisteredAnnounceAppData,
    ) -> Result<(), super::SetRegisteredAnnounceAppDataError> {
        self.set_registered_announce_app_data(set).await
    }

    async fn send_single_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>> {
        self.send_single_packet(destination, data).await
    }

    async fn send_plain_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<(), SendError<SendPlainPacketFailure>> {
        self.send_plain_packet(destination, data).await
    }

    async fn send_group_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<(), SendError<SendGroupFailure>> {
        self.send_group_packet(destination, data).await
    }

    fn respond_packed(&self, responder: RespondToken, packed: &[u8]) -> bool {
        self.respond_packed(responder, packed).is_some()
    }

    fn close_link(&self, link_id: LinkId) -> bool {
        self.close_link(link_id)
    }
}

#[cfg(test)]
mod tests;
