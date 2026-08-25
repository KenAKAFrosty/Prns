use crate::engine::{
    AnnounceAppData, AnnounceNow, AnnounceNowFailure, AnnounceNowRejection, AnnounceTarget,
    AnnounceWriteFailure, CommandId, PacketReceiptDelivered, PrnsCommand, SendGroupFailure,
    SendPlainPacketFailure, SendSinglePacketFailure,
};
pub use crate::engine::{DropRouteOutcome, DropRoutesViaOutcome};
use crate::identity::{
    IdentityHash, MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
    RetainIdentityOutcome,
};
use crate::routing::links::LinkId;
use crate::wire::{DestinationHash, TransportId};

use super::request_endpoints::RespondToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClearAnnounceQueuesOutcome {
    pub dropped_announces: u32,
}

/// Why an awaited send never reached `Delivered`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError<F> {
    PayloadTooLarge,
    NodeStopped,
    /// More awaited sends are in flight than the platform tracks at once
    Busy,
    Failed(F),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceNowError {
    NodeStopped,
    Busy,
    Rejected(AnnounceNowRejection),
    WriteFailed(AnnounceWriteFailure),
}

impl AnnounceNowError {
    #[must_use]
    pub const fn from_failure(failure: AnnounceNowFailure) -> Self {
        match failure {
            AnnounceNowFailure::Rejected(rejection) => Self::Rejected(rejection),
            AnnounceNowFailure::WriteFailed(failure) => Self::WriteFailed(failure),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingControlError {
    NodeStopped,
    Busy,
}

pub trait RoutingControl {
    fn drop_route(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<Output = Result<DropRouteOutcome, RoutingControlError>> + Send;

    fn drop_routes_via(
        &self,
        transport: TransportId,
    ) -> impl core::future::Future<Output = Result<DropRoutesViaOutcome, RoutingControlError>> + Send;

    fn clear_announce_queues(
        &self,
    ) -> impl core::future::Future<Output = Result<ClearAnnounceQueuesOutcome, RoutingControlError>> + Send;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationIdentityRetentionControlError {
    NodeStopped,
    Busy,
}

pub trait DestinationIdentityRetentionControl {
    fn mark_destination_used(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<
        Output = Result<MarkDestinationUsedOutcome, DestinationIdentityRetentionControlError>,
    > + Send;

    fn retain_destination(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<
        Output = Result<RetainDestinationOutcome, DestinationIdentityRetentionControlError>,
    > + Send;

    fn release_destination(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<
        Output = Result<ReleaseDestinationOutcome, DestinationIdentityRetentionControlError>,
    > + Send;

    fn retain_identity(
        &self,
        identity: IdentityHash,
    ) -> impl core::future::Future<
        Output = Result<RetainIdentityOutcome, DestinationIdentityRetentionControlError>,
    > + Send;
}

/// The high-level API shared by every platform's node handle. Tokio carries commands over an unbounded channel with per-command oneshots; Embassy uses a bounded channel and static completion pool.
///
/// [`issue`](Self::issue) returns a minted [`CommandId`] immediately, while awaited operations settle through that id. Platform-specific capabilities remain inherent methods on the concrete handle.
#[allow(async_fn_in_trait)]
pub trait PrnsNodeApi {
    /// Queues an engine command and returns the [`CommandId`] it was minted under. If you're looking for its settlement, watch the event stream for the settlement tagged with that CommandId.
    ///
    /// You may not ever need this, since many operations have their own convenience methods, usually `await`able.
    fn issue(&self, command: PrnsCommand) -> Option<CommandId>;

    /// Announce `destination` on every interface with its registered app_data: RNS 1.4.2 `Destination.announce()` with no arguments.
    ///
    /// Returns `None` if the node has stopped. If you want to target a specific interface or provide explicit app_data, [`issue`](Self::issue) a custom [`AnnounceNow`]. Some platform implementations may provide an awaitable convenience method, specifically for `announce_now`, on top of this.
    fn announce(&self, destination: DestinationHash) -> Option<CommandId> {
        self.issue(PrnsCommand::AnnounceNow(AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        }))
    }

    async fn announce_now(&self, announce: AnnounceNow) -> Result<(), AnnounceNowError>;

    async fn send_single_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>>;

    async fn send_plain_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<(), SendError<SendPlainPacketFailure>>;

    async fn send_group_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<(), SendError<SendGroupFailure>>;

    fn respond_packed(&self, responder: RespondToken, packed: &[u8]) -> bool;

    fn close_link(&self, link_id: LinkId) -> bool;
}

#[cfg(test)]
impl PrnsNodeApi for () {
    fn issue(&self, _command: PrnsCommand) -> Option<CommandId> {
        None
    }

    async fn announce_now(&self, _announce: AnnounceNow) -> Result<(), AnnounceNowError> {
        Err(AnnounceNowError::NodeStopped)
    }

    async fn send_single_packet(
        &self,
        _destination: DestinationHash,
        _data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>> {
        Err(SendError::NodeStopped)
    }

    async fn send_plain_packet(
        &self,
        _destination: DestinationHash,
        _data: &[u8],
    ) -> Result<(), SendError<SendPlainPacketFailure>> {
        Err(SendError::NodeStopped)
    }

    async fn send_group_packet(
        &self,
        _destination: DestinationHash,
        _data: &[u8],
    ) -> Result<(), SendError<SendGroupFailure>> {
        Err(SendError::NodeStopped)
    }

    fn respond_packed(&self, _responder: RespondToken, _packed: &[u8]) -> bool {
        false
    }

    fn close_link(&self, _link_id: LinkId) -> bool {
        false
    }
}
